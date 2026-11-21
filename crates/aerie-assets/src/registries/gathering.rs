//! What breaking a block takes and gives (the official server's block
//! damage, read from the assets):
//!
//! - a block's `Gathering.Breaking`: `GatherType`, `Quality` (0), the item it
//!   drops (`ItemId`, `Quantity` 1) or a `DropList`; `Gathering.Soft` blocks
//!   break in one hit;
//! - an item's `Tool.Specs`: per `GatherType`, `Power` and `Quality`;
//! - bare hands: `Item/Unarmed/Gathering/<GatherType>` (`Power`, `Quality`).
//!
//! A block has health 1; each hit takes the power of the held tool's spec
//! for the block's gather type (or the bare-hand spec), if its quality is
//! at least the block's. Written as text lines for the start-up cache:
//! `B key gather quality item quantity soft soft_item`, `T item gather power
//! quality`, `U gather power quality` (`-` for none).

use serde_json::Value;

use crate::Result;
use crate::ctx::Ctx;
use crate::values::Get;

pub const UNARMED: &str = "Item/Unarmed/Gathering";

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
        let Some(g) = a.obj("Gathering") else {
            continue;
        };
        let b = g.obj("Breaking");
        let soft = g.obj("Soft");
        if b.is_none() && soft.is_none() {
            continue;
        }
        let nul = Value::Null;
        let b = b.unwrap_or(&nul);
        out += &format!(
            "B {k} {} {} {} {} {} {}\n",
            or_dash(b.str_at("GatherType")),
            b.i32_or("Quality", 0),
            or_dash(b.str_at("ItemId")),
            b.i32_or("Quantity", 1),
            soft.is_some() as u8,
            or_dash(soft.and_then(|s| s.str_at("ItemId"))),
        );
    }
    let items = ctx.store(super::items::PATH)?;
    let mut keys: Vec<&String> = items.own.keys().collect();
    keys.sort();
    for k in keys {
        let a = items.resolved(k, "Item");
        for s in a
            .obj("Tool")
            .and_then(|t| t.get("Specs"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if s.bool_or("IsIncorrect", false) {
                continue;
            }
            if let Some(gt) = s.str_at("GatherType") {
                out += &format!(
                    "T {k} {gt} {} {}\n",
                    s.f32_or("Power", 0.0),
                    s.i32_or("Quality", 0)
                );
            }
        }
    }
    let unarmed = ctx.store(UNARMED)?;
    let mut keys: Vec<&String> = unarmed.own.keys().collect();
    keys.sort();
    for k in keys {
        let a = unarmed.resolved(k, "ItemToolSpec");
        let gt = a.str_at("GatherType").unwrap_or_else(|| k.clone());
        out += &format!(
            "U {gt} {} {}\n",
            a.f32_or("Power", 0.0),
            a.i32_or("Quality", 0)
        );
    }
    Ok(out)
}
