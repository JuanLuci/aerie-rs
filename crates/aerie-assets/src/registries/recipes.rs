//! `UpdateRecipes` (60): crafting recipes by id (spec 07), from
//! `Item/Recipes` files and from items with a `Recipe`.
//!
//! A recipe without outputs gets its `PrimaryOutput` as the only output. An
//! item's `Recipe` (file and contained items alike) becomes
//! `<item>_Recipe_Generated_0`: its primary output is the recipe's single
//! `Output` when that names the item, else `{ItemId: <item>, Quantity:
//! OutputQuantity (1)}`, which also becomes the only output when there is
//! none.
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | `Input` (materials) | `inputs` | absent when empty |
//! | `Output` (materials) | `outputs` | see above |
//! | `PrimaryOutput` | `primary_output` | absent |
//! | `BenchRequirement` `[{Type, Id, Categories, RequiredTierLevel, RequiredAugmentTags}]` | same | absent when empty |
//! | `KnowledgeRequired` | same | false |
//! | `TimeSeconds` | `time_seconds` | 0 |
//! | `RequiredMemoriesLevel` | same | 1 |
//!
//! Material: `ItemId`, `ItemTag` (→ tag index, `i32::MIN` when absent),
//! `ResourceTypeId`, `Quantity` (1).

use aerie_protocol::proto::{
    BenchRequirement, BenchType, CraftingRecipe, MaterialQuantity, UpdateRecipes,
};
use aerie_protocol::wire::Wire;
use serde_json::Value;

use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const PATH: &str = "Item/Recipes";
pub const ITEMS: &str = "Item/Items";

pub fn material(ctx: &Ctx, m: &Value) -> MaterialQuantity {
    MaterialQuantity {
        item_id: m.str_at("ItemId"),
        item_tag: m.str_at("ItemTag").map_or(i32::MIN, |t| ctx.tag(&t)),
        resource_type_id: m.str_at("ResourceTypeId"),
        quantity: m.i32_or("Quantity", 1),
    }
}

fn materials(ctx: &Ctx, v: &Value, key: &str) -> Option<Vec<MaterialQuantity>> {
    v.get(key)?
        .as_array()
        .filter(|a| !a.is_empty())
        .map(|a| a.iter().map(|m| material(ctx, m)).collect())
}

/// A recipe; `item` is the item key for a recipe written in an item.
fn recipe(ctx: &Ctx, id: &str, r: &Value, item: Option<&str>) -> CraftingRecipe {
    let mut primary = r.obj("PrimaryOutput").map(|m| material(ctx, m));
    // Decoding a recipe already fills missing outputs with the primary one.
    let mut outputs = materials(ctx, r, "Output").or_else(|| primary.clone().map(|p| vec![p]));
    match item {
        Some(item) => {
            primary = match &outputs {
                Some(o) if o.len() == 1 && o[0].item_id.as_deref() == Some(item) => {
                    Some(o[0].clone())
                }
                _ => {
                    let p = MaterialQuantity {
                        item_id: Some(item.to_owned()),
                        item_tag: i32::MIN,
                        resource_type_id: None,
                        quantity: r.i32_or("OutputQuantity", 1),
                    };
                    if outputs.is_none() {
                        outputs = Some(vec![p.clone()]);
                    }
                    Some(p)
                }
            };
        }
        None => {}
    }
    CraftingRecipe {
        id: Some(id.to_owned()),
        inputs: materials(ctx, r, "Input"),
        outputs,
        primary_output: primary,
        bench_requirement: r
            .get("BenchRequirement")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .map(|a| {
                a.iter()
                    .map(|b| BenchRequirement {
                        r#type: enum_or(b, "Type", BenchType::from_name, BenchType::CRAFTING),
                        id: b.str_at("Id").unwrap_or_default(),
                        categories: b.strings("Categories"),
                        required_tier_level: b.i32_or("RequiredTierLevel", 0),
                        required_augment_tags: b.strings("RequiredAugmentTags"),
                    })
                    .collect()
            }),
        knowledge_required: r.bool_or("KnowledgeRequired", false),
        time_seconds: r.f32_or("TimeSeconds", 0.0),
        required_memories_level: r.i32_or("RequiredMemoriesLevel", 1),
    }
}

/// Every recipe as (id, JSON, item it was written in).
fn all(ctx: &Ctx) -> Result<Vec<(String, Value, Option<String>)>> {
    let store = ctx.store(PATH)?;
    let mut out: Vec<(String, Value, Option<String>)> = store
        .own
        .keys()
        .map(|k| (k.clone(), store.resolved(k, "CraftingRecipe"), None))
        .collect();
    let items = ctx.store(ITEMS)?;
    for (k, v) in &items.own {
        if let Some(r) = v.get("Recipe").filter(|r| r.is_object()) {
            out.push((
                format!("{k}_Recipe_Generated_0"),
                r.clone(),
                Some(k.clone()),
            ));
        }
    }
    Ok(out)
}

pub fn build(ctx: &Ctx) -> Result<UpdateRecipes> {
    let list = all(ctx)?
        .iter()
        .map(|(id, r, item)| (id.clone(), recipe(ctx, id, r, item.as_deref())))
        .collect();
    Ok(UpdateRecipes {
        r#type: super::INIT,
        recipes: Some(list),
        removed_recipes: None,
    })
}

/// Compare runs: each `ItemTag` of a captured recipe gives its tag index.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    let (cap, _) = <UpdateRecipes as Wire>::decode(captured, 0)?;
    let ours: std::collections::HashMap<String, Value> =
        all(ctx)?.into_iter().map(|(id, r, _)| (id, r)).collect();
    for (id, c) in cap.recipes.unwrap_or_default() {
        let Some(r) = ours.get(&id) else { continue };
        for (key, list) in [("Input", c.inputs), ("Output", c.outputs)] {
            let json = r.get(key).and_then(Value::as_array);
            for (j, m) in json.into_iter().flatten().zip(list.into_iter().flatten()) {
                if let Some(t) = j.str_at("ItemTag") {
                    ctx.pin_tag(&t, m.item_tag);
                }
            }
        }
    }
    Ok(())
}
