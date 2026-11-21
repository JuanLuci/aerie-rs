//! `UpdateBlockHitboxes` (41) from `Item/Block/Hitboxes`, indexed, built-in
//! `Full` (the unit box) first (spec 07). `Boxes` (inherited) is a list of
//! `{Min: {X, Y, Z}, Max: {X, Y, Z}}`, sent as is: the rotated variants are
//! computed on each side. The packet carries no ids.

use aerie_protocol::proto::{Hitbox, UpdateBlockHitboxes};
use serde_json::Value;

use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub const PATH: &str = "Item/Block/Hitboxes";
pub const BUILTIN: &[&str] = &["Full"];

fn boxes(a: &Value) -> Vec<Hitbox> {
    a.get("Boxes")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .map(|b| {
                    let c = |side: &str, axis: &str| {
                        b.obj(side).map(|v| v.f32_or(axis, 0.0)).unwrap_or(0.0)
                    };
                    Hitbox {
                        min_x: c("Min", "X"),
                        min_y: c("Min", "Y"),
                        min_z: c("Min", "Z"),
                        max_x: c("Max", "X"),
                        max_y: c("Max", "Y"),
                        max_z: c("Max", "Z"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn build(ctx: &Ctx) -> Result<UpdateBlockHitboxes> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    let list = order
        .iter()
        .enumerate()
        .map(|(n, k)| {
            let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
            let b = if store.own.contains_key(k) {
                boxes(&store.resolved(k, "BlockBoundingBoxes"))
            } else {
                vec![Hitbox {
                    max_x: 1.0,
                    max_y: 1.0,
                    max_z: 1.0,
                    ..Default::default()
                }]
            };
            (i, b)
        })
        .collect();
    Ok(UpdateBlockHitboxes {
        r#type: super::INIT,
        max_id: order.len() as i32,
        block_base_hitboxes: Some(list),
    })
}
