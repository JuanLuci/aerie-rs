//! Prints a built registry as canonical debug text (maps in key order).
//!
//! cargo run --release -p aerie-assets --example dump_built -- <packet id> [--pin <capture dir>]
//! cargo run --release -p aerie-assets --example dump_built -- <packet id> --captured <file>
//!
//! `--pin` first uses a probe capture's indexes and tags, like `compare`
//! (then `--keys <store path>` prints index → key for that folder instead);
//! `--captured` prints a captured payload in the same canonical form.

use std::path::PathBuf;

use aerie_assets::{ctx::Ctx, pack::Pack, registries};

fn main() {
    let mut args = std::env::args().skip(1);
    let id: u32 = args.next().expect("packet id").parse().unwrap();
    let reg = registries::all()
        .into_iter()
        .find(|r| r.id == id)
        .expect("no such registry");
    let next = args.next();
    if next.as_deref() == Some("--captured") {
        let bytes = std::fs::read(args.next().expect("file")).unwrap();
        println!("{}", (reg.canonical)(&bytes).unwrap());
        return;
    }
    let zip = Pack::default_path().expect("Assets.zip");
    let ctx = Ctx::new(Pack::open(&zip).unwrap());
    if next.as_deref() == Some("--pin") {
        let dir = PathBuf::from(args.next().expect("capture dir"));
        let index = std::fs::read_to_string(dir.join("index.tsv")).unwrap();
        let captured = |id: u32| -> Option<Vec<u8>> {
            index
                .lines()
                .filter(|l| l.split('\t').nth(1) == Some(&id.to_string()))
                .filter_map(|line| {
                    let c: Vec<&str> = line.split('\t').collect();
                    std::fs::read(dir.join(format!(
                        "{:04}-{}-{}.bin",
                        c[0].parse::<u32>().unwrap(),
                        c[1],
                        c[2]
                    )))
                    .ok()
                })
                .max_by_key(Vec::len)
        };
        registries::pin_from_capture(&ctx, captured).unwrap();
        if let (Some("--keys"), Some(path)) = (args.next().as_deref(), args.next()) {
            for (i, k) in ctx.order(&path, &[]).unwrap().iter().enumerate() {
                let idx = ctx.indexes(&path, &[]).unwrap();
                println!(
                    "{}	{k}",
                    idx.get(&k.to_lowercase()).copied().unwrap_or(-(i as i32))
                );
            }
            return;
        }
    }
    let bytes = (reg.build)(&ctx).unwrap();
    println!("{}", (reg.canonical)(&bytes).unwrap());
}
