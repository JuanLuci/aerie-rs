//! Lists the contained assets found in an installation (spec 07).
//!
//! cargo run --release -p aerie-assets --example contained -- [type]
//!
//! Without a type: the count per asset type. With one: that type's keys.
//! `--order <store path>`: a folder's keys in index order.
//! `--tags <store path> <key>`: an asset's tags.

use aerie_assets::{ctx::Ctx, pack::Pack};

fn main() {
    let ty = std::env::args().nth(1);
    let pack = Pack::open(&Pack::default_path().expect("Assets.zip")).unwrap();
    let ctx = Ctx::new(pack);
    if ty.as_deref() == Some("--tags") {
        let path = std::env::args().nth(2).expect("store path");
        let key = std::env::args().nth(3).expect("key");
        let store = ctx.store(&path).unwrap();
        let t = aerie_assets::tags::of(&store, &key);
        println!(
            "raw {:?}
expanded {:?}",
            t.raw, t.expanded
        );
        return;
    }
    if ty.as_deref() == Some("--order") {
        let path = std::env::args().nth(2).expect("store path");
        for (i, k) in ctx.order(&path, &[]).unwrap().iter().enumerate() {
            println!("{i}	{k}");
        }
        return;
    }
    let t = std::time::Instant::now();
    let all = aerie_assets::contained::discover_ctx(&ctx).unwrap();
    eprintln!("{} contained assets in {:?}", all.len(), t.elapsed());
    match ty {
        None => {
            let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
            for c in &all {
                *counts.entry(c.class.as_str()).or_default() += 1;
            }
            for (k, n) in counts {
                println!("{n:6} {k}");
            }
        }
        Some(ty) => {
            for c in all.iter().filter(|c| c.class == ty) {
                println!("{}\t{}", c.key, c.parent.as_deref().unwrap_or("-"));
            }
        }
    }
}
