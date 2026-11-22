//! What block physics needs beyond the client's block registry (spec 10):
//! per block with any of them, `SupportDropType`, the `Gathering.Physics`
//! drop and `Gathering.UseDefaultDropWhenPlaced`; and every drop list.
//!
//! Falling blocks (spec 24) add `F key impact dropItems collision`.
//!
//! Text lines for the start-up cache: `P key drop item list deco`
//! (drop: 0 Break, 1 Destroy, 2 Fall; `-` for none; deco 1 when placed
//! blocks become decoration) and `L list <json>` (the list's `Container`,
//! one line of JSON).

use serde_json::Value;

use super::music::{owner_of, reference};
use crate::Result;
use crate::ctx::Ctx;
use crate::values::Get;

pub const DROPS: &str = "Drops";

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
        // A door, and whether it is a horizontal one (its use
        // interaction says so): `D key horizontal`.
        if a.bool_or("IsDoor", false) {
            let horizontal = a
                .obj("Interactions")
                .and_then(|i| i.str_at("Use"))
                .is_some_and(|u| u.contains("Horizontal"));
            out += &format!("D {k} {}\n", horizontal as u8);
        }
        // The sound of each interaction state: `S key state sound`.
        for (state, def) in a
            .obj("State")
            .and_then(|s| s.obj("Definitions"))
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if state.contains(char::is_whitespace) {
                continue;
            }
            let sound =
                super::sound_events::index(ctx, def.str_at("InteractionSoundEventId").as_deref())?;
            if sound > 0 {
                out += &format!("S {k} {state} {sound}\n");
            }
        }
        // A block that holds items: `K key capacity stayOpenWhenEmpty
        // openSound closeSound` (the sounds of its window states).
        if let Some(c) = a
            .obj("BlockEntity")
            .and_then(|e| e.obj("Components"))
            .and_then(|c| c.obj("ItemContainerBlock"))
        {
            let sound = |state: &str| -> Result<i32> {
                let id = a
                    .obj("State")
                    .and_then(|s| s.obj("Definitions"))
                    .and_then(|d| d.obj(state))
                    .and_then(|s| s.str_at("InteractionSoundEventId"));
                super::sound_events::index(ctx, id.as_deref())
            };
            out += &format!(
                "K {k} {} {} {} {}\n",
                c.get("Capacity").and_then(Value::as_i64).unwrap_or(20),
                c.bool_or("StayOpenWhenEmpty", false) as u8,
                sound("OpenWindow")?,
                sound("CloseWindow")?
            );
        }
        // What a falling block does when it lands: `F key impact
        // dropItems collision` (impact: 0 Place, 1 Break, 2 Explode;
        // collision: the hitbox collision config's index, -1 for none).
        if let Some(f) = a.obj("FallingBlockSettings") {
            let impact = f.obj("Impact");
            let kind = match impact.and_then(|i| i.str_at("Type")).as_deref() {
                Some("Break") => 1,
                Some("Explode") => 2,
                _ => 0,
            };
            let drops = impact.is_none_or(|i| i.bool_or("DropItems", true));
            let collision = ctx.index_or(
                super::entity_configs::HITBOX,
                &[],
                f.str_at("HitboxCollisionConfig").as_deref(),
                -1,
            )?;
            out += &format!("F {k} {kind} {} {collision}\n", drops as u8);
        }
        let drop = match a.str_at("SupportDropType").as_deref() {
            Some("Destroy") => 1,
            Some("Fall") => 2,
            _ => 0,
        };
        let g = a.obj("Gathering");
        let physics = g.and_then(|g| g.obj("Physics"));
        let deco = a.bool_or("IgnoreSupportWhenPlaced", false)
            || g.is_some_and(|g| g.bool_or("UseDefaultDropWhenPlaced", false));
        if drop == 0 && physics.is_none() && !deco {
            continue;
        }
        let owner = owner_of(&blocks, k, "Gathering");
        let (item, list) = match physics {
            Some(p) => (
                p.str_at("ItemId"),
                p.get("DropList")
                    .and_then(|d| reference(d, &owner, "Gathering_Physics_DropList")),
            ),
            None => (None, None),
        };
        // `-` in the item column with a physics entry means "no item".
        let item = match (physics.is_some(), item) {
            (true, None) => "!".to_owned(),
            (_, i) => or_dash(i),
        };
        out += &format!("P {k} {drop} {item} {} {}\n", or_dash(list), deco as u8);
    }
    let lists = ctx.store(DROPS)?;
    let mut keys: Vec<&String> = lists.own.keys().collect();
    keys.sort();
    for k in keys {
        if k.contains(char::is_whitespace) {
            continue;
        }
        let a = lists.resolved(k, "ItemDropList");
        let c = a.get("Container").cloned().unwrap_or(Value::Null);
        out += &format!("L {k} {}\n", c);
    }
    Ok(out)
}
