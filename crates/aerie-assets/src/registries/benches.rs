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
//!
//! Processing benches (spec 16) add `P block maxFuel fuelDropItem
//! allowNoInput outputSlots endSound`, per input slot `PI block icon
//! resource filterValid`, per fuel slot `PF block icon resource`, `PX block
//! perFuelItems outputs ignored` (comma lists, outputs as `item:qty`) for
//! `ExtraOutput`, and per tier `PT block extraInput extraOutput reduction`;
//! items whose `FuelQuality` is not 1 give `Q item quality`. An item whose
//! interactions have a `LearnRecipe` step that names an item gives `L item
//! learned` (spec 09, known recipes).

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
        if ty == BenchType::PROCESSING {
            out += &format!(
                "P {k} {} {} {} {} {}\n",
                b.i32_or("MaxFuel", 0),
                or_dash(b.str_at("FuelDropItemId")),
                b.bool_or("AllowNoInputProcessing", false) as u8,
                b.i32_or("OutputSlotsCount", 1),
                sound("EndSoundEventId")?,
            );
            for s in b
                .get("Input")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out += &format!(
                    "PI {k} {} {} {}\n",
                    or_dash(s.str_at("Icon")),
                    or_dash(s.str_at("ResourceTypeId")),
                    s.bool_or("FilterValidIngredients", false) as u8,
                );
            }
            for s in b
                .get("Fuel")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out += &format!(
                    "PF {k} {} {}\n",
                    or_dash(s.str_at("Icon")),
                    or_dash(s.str_at("ResourceTypeId")),
                );
            }
            if let Some(x) = b.obj("ExtraOutput") {
                let list = |key: &str, q: bool| -> String {
                    let v: Vec<String> = x
                        .get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|m| {
                            let id = m.str_at("ItemId")?;
                            Some(if q {
                                format!("{id}:{}", m.i32_or("Quantity", 1))
                            } else {
                                id
                            })
                        })
                        .collect();
                    if v.is_empty() {
                        "-".into()
                    } else {
                        v.join(",")
                    }
                };
                out += &format!(
                    "PX {k} {} {} {}\n",
                    x.i32_or("PerFuelItemsConsumed", 1),
                    list("Outputs", true),
                    list("IgnoredFuelSources", false),
                );
            }
            for t in b
                .get("TierLevels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out += &format!(
                    "PT {k} {} {} {}\n",
                    t.i32_or("ExtraInputSlot", 0),
                    t.i32_or("ExtraOutputSlot", 0),
                    t.f64_or("CraftingTimeReductionModifier", 0.0),
                );
            }
        }
    }
    let items = ctx.store(super::items::PATH)?;
    for k in items.own.keys() {
        let a = items.resolved(k, "Item");
        let q = a.f64_or("FuelQuality", 1.0);
        if q != 1.0 {
            out += &format!("Q {k} {q}\n");
        }
    }
    for k in items.own.keys() {
        let a = items.resolved(k, "Item");
        if let Some(learned) = a.get("Interactions").and_then(learned_item) {
            out += &format!("L {k} {learned}\n");
        }
    }
    for name in SOUNDS {
        let i = super::sound_events::index(ctx, Some(name))?;
        if i > 0 {
            out += &format!("S {name} {i}\n");
        }
    }
    Ok(out)
}

/// The item a `LearnRecipe` interaction names, looked for anywhere in an
/// item's interactions (it can sit inside other steps).
fn learned_item(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => {
            if m.get("Type").and_then(Value::as_str) == Some("LearnRecipe")
                && let Some(id) = m.get("ItemId").and_then(Value::as_str)
                && !id.contains(char::is_whitespace)
            {
                return Some(id.to_owned());
            }
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            keys.into_iter().find_map(|k| learned_item(&m[k]))
        }
        Value::Array(a) => a.iter().find_map(learned_item),
        _ => None,
    }
}

/// Sounds the official crafting plays outside bench assets.
pub const SOUNDS: &[&str] = &[
    "SFX_Player_Craft_Item_Inventory",
    "SFX_Player_Pickup_Item",
    "SFX_Player_Drop_Item",
    "SFX_Item_Break",
];
