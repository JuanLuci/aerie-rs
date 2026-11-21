//! `UpdateItemCategories` (56) from `Server/Item/Category/CreativeLibrary` and
//! `UpdateFieldcraftCategories` (58) from `Server/Item/Category/Fieldcraft` (spec 07).
//! Both are plain lists (no indexes).
//!
//! Creative library category: `Name`, `Icon`, `Order` (0), `InfoDisplayMode`
//! (Tooltip), `Children` (categories of the same shape, with their own `Id`)
//! and `SubCategories` (`Id`, `Name`, `Description`, `Order`). Empty lists are
//! left out. Fieldcraft category: `Name`, `Icon`, `Order`.

use aerie_protocol::proto::{
    ItemCategory, ItemGridInfoDisplayMode, SubCategoryDefinition, UpdateFieldcraftCategories,
    UpdateItemCategories,
};
use serde_json::Value;

use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const CREATIVE: &str = "Item/Category/CreativeLibrary";
pub const FIELDCRAFT: &str = "Item/Category/Fieldcraft";

fn non_empty<T>(v: Vec<T>) -> Option<Vec<T>> {
    (!v.is_empty()).then_some(v)
}

fn category(id: Option<String>, a: &Value) -> ItemCategory {
    let arr = |k: &str| {
        a.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    ItemCategory {
        id,
        name: a.str_at("Name"),
        icon: a.str_at("Icon"),
        order: a.i32_or("Order", 0),
        info_display_mode: enum_or(
            a,
            "InfoDisplayMode",
            ItemGridInfoDisplayMode::from_name,
            ItemGridInfoDisplayMode::TOOLTIP,
        ),
        children: non_empty(
            arr("Children")
                .iter()
                .map(|c| category(c.str_at("Id"), c))
                .collect(),
        ),
        sub_categories: non_empty(
            arr("SubCategories")
                .iter()
                .map(|s| SubCategoryDefinition {
                    id: s.str_at("Id"),
                    name: s.str_at("Name"),
                    description: s.str_at("Description"),
                    order: s.i32_or("Order", 0),
                })
                .collect(),
        ),
    }
}

pub fn build(ctx: &Ctx) -> Result<UpdateItemCategories> {
    let store = ctx.store(CREATIVE)?;
    let list = store
        .assets
        .iter()
        .map(|(k, a)| category(Some(k.clone()), a))
        .collect();
    Ok(UpdateItemCategories {
        r#type: super::INIT,
        item_categories: Some(list),
    })
}

pub fn build_fieldcraft(ctx: &Ctx) -> Result<UpdateFieldcraftCategories> {
    let store = ctx.store(FIELDCRAFT)?;
    let list = store
        .assets
        .iter()
        .map(|(k, a)| ItemCategory {
            id: Some(k.clone()),
            name: a.str_at("Name"),
            icon: a.str_at("Icon"),
            order: a.i32_or("Order", 0),
            ..Default::default()
        })
        .collect();
    Ok(UpdateFieldcraftCategories {
        r#type: super::INIT,
        item_categories: Some(list),
    })
}
