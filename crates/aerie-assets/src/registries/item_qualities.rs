//! `UpdateItemQualities` (55) from `Server/Item/Qualities` (spec 07).
//!
//! Built-in entry at index 0: `Default` (values below).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | file name | `id` | |
//! | `ItemTooltipTexture`, `ItemTooltipArrowTexture`, `SlotTexture`, `BlockSlotTexture`, `SpecialSlotTexture` | same names | absent |
//! | `TextColor` (color) | `text_color` | absent |
//! | `LocalizationKey` | `localization_key` | absent |
//! | `VisibleQualityLabel`, `RenderSpecialSlot`, `HideFromSearch` | same names | `false` |
//!
//! `QualityValue` and `ItemEntityConfig` stay on the server.

use aerie_protocol::proto::{ItemQuality, UpdateItemQualities};
use serde_json::Value;

use crate::values::{Get, parse_color};
use crate::{Result, ctx::Ctx};

pub const PATH: &str = "Item/Qualities";
pub const BUILTIN: &[&str] = &["Default"];

fn default_entry() -> ItemQuality {
    let s = |x: &str| Some(x.to_owned());
    ItemQuality {
        id: s("Default"),
        item_tooltip_texture: s("UI/ItemQualities/Tooltips/ItemTooltipDefault.png"),
        item_tooltip_arrow_texture: s("UI/ItemQualities/Tooltips/ItemTooltipDefaultArrow.png"),
        slot_texture: s("UI/ItemQualities/Slots/SlotDefault.png"),
        block_slot_texture: s("UI/ItemQualities/Slots/SlotDefault.png"),
        special_slot_texture: s("UI/ItemQualities/Slots/SpecialSlotDefault.png"),
        text_color: parse_color("#c9d2dd"),
        localization_key: s("server.general.qualities.Default"),
        visible_quality_label: false,
        render_special_slot: false,
        hide_from_search: false,
    }
}

fn entry(id: &str, a: &Value) -> ItemQuality {
    ItemQuality {
        id: Some(id.to_owned()),
        item_tooltip_texture: a.str_at("ItemTooltipTexture"),
        item_tooltip_arrow_texture: a.str_at("ItemTooltipArrowTexture"),
        slot_texture: a.str_at("SlotTexture"),
        block_slot_texture: a.str_at("BlockSlotTexture"),
        special_slot_texture: a.str_at("SpecialSlotTexture"),
        text_color: a.color("TextColor"),
        localization_key: a.str_at("LocalizationKey"),
        visible_quality_label: a.bool_or("VisibleQualityLabel", false),
        render_special_slot: a.bool_or("RenderSpecialSlot", false),
        hide_from_search: a.bool_or("HideFromSearch", false),
    }
}

pub fn build(ctx: &Ctx) -> Result<UpdateItemQualities> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let map = order
        .iter()
        .enumerate()
        .map(|(i, k)| {
            (
                i as i32,
                if k == "Default" {
                    default_entry()
                } else {
                    entry(k, &store.assets[k])
                },
            )
        })
        .collect();
    Ok(UpdateItemQualities {
        r#type: super::INIT,
        max_id: order.len() as i32,
        item_qualities: Some(map),
    })
}
