//! Crafting benches, for the server's bench windows: a block's `Bench`
//! (`Type`, `Id`, `Categories` with `Id`, `Icon`, `Name`).
//!
//! Written as text lines for the start-up cache: `W block type id open
//! close completed failed` (type is the `BenchType` number, then the sound
//! event indexes of `LocalOpenSoundEventId`, `LocalCloseSoundEventId`,
//! `CompletedSoundEventId` and `FailedSoundEventId`, 0 for none), per category in order `C block id
//! icon name` (`-` for none; structural benches list names only), `H block
//! category` per header category, `F block cycling hints` (structural
//! benches' `AllowBlockGroupCycling` and `AlwaysShowInventoryHints`), and
//! `S name index` for the sounds crafting plays outside benches.

use aerie_protocol::proto::BenchType;
use serde_json::Value;

use crate::Result;
use crate::ctx::Ctx;
use crate::values::Get;

fn or_dash(s: Option<String>) -> String {
    s.filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
        .unwrap_or_else(|| "-".into())
}

pub fn table(ctx: &Ctx) -> Result<String> {
    let mut out = String::new();
    let blocks = ctx.store(super::blocks::PATH)?;
    let mut keys: Vec<&String> = blocks.own.keys().collect();
    keys.sort();
    for k in keys {
        let a = blocks.resolved(k, "BlockType");
        let Some(b) = a.obj("Bench") else { continue };
        let Some(id) = b.str_at("Id").filter(|s| !s.contains(char::is_whitespace)) else {
            continue;
        };
        let ty = b
            .str_at("Type")
            .and_then(|t| BenchType::from_name(&t))
            .unwrap_or(BenchType::CRAFTING);
        let sound = |key: &str| -> Result<i32> {
            Ok(match b.str_at(key) {
                Some(n) => super::sound_events::index(ctx, Some(&n))?.max(0),
                None => 0,
            })
        };
        out += &format!(
            "W {k} {} {id} {} {} {} {}\n",
            ty.0,
            sound("LocalOpenSoundEventId")?,
            sound("LocalCloseSoundEventId")?,
            sound("CompletedSoundEventId")?,
            sound("FailedSoundEventId")?,
        );
        for c in b
            .get("Categories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match c {
                // Structural benches list plain category names.
                Value::String(id) => out += &format!("C {k} {} - -\n", or_dash(Some(id.clone()))),
                _ => {
                    out += &format!(
                        "C {k} {} {} {}\n",
                        or_dash(c.str_at("Id")),
                        or_dash(c.str_at("Icon")),
                        or_dash(c.str_at("Name")),
                    )
                }
            }
        }
        for h in b.strings("HeaderCategories").unwrap_or_default() {
            out += &format!("H {k} {}\n", or_dash(Some(h)));
        }
        out += &format!(
            "F {k} {} {}\n",
            b.bool_or("AllowBlockGroupCycling", false) as u8,
            b.bool_or("AlwaysShowInventoryHints", false) as u8
        );
    }
    for name in SOUNDS {
        let i = super::sound_events::index(ctx, Some(name))?;
        if i > 0 {
            out += &format!("S {name} {i}\n");
        }
    }
    Ok(out)
}

/// Sounds the official crafting plays outside bench assets.
pub const SOUNDS: &[&str] = &[
    "SFX_Player_Craft_Item_Inventory",
    "SFX_Player_Pickup_Item",
    "SFX_Player_Drop_Item",
    "SFX_Item_Break",
];
