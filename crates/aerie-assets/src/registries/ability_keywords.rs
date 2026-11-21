//! `UpdateAbilityKeywords` (94) from `Server/Abilities/Keywords` (spec 07).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | file name | `id` | |
//! | `Name` | `translation_key` | required |
//! | `ShowOnTooltip` | `show_on_tooltip` | `true` |
//!
//! `RequiredAttributes` and `RequiredInteractions` stay on the server.

use aerie_protocol::proto::{AbilityKeyword, UpdateAbilityKeywords};

use crate::values::Get;
use crate::{Error, Result, ctx::Ctx};

pub const PATH: &str = "Abilities/Keywords";

pub fn build(ctx: &Ctx) -> Result<UpdateAbilityKeywords> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, &[])?;
    let mut map = Vec::new();
    for (i, key) in order.iter().enumerate() {
        let a = &store.assets[key];
        let name = a.str_at("Name").ok_or_else(|| Error::Invalid {
            asset: key.clone(),
            msg: "Name is required".into(),
        })?;
        map.push((
            i as i32,
            AbilityKeyword {
                id: Some(key.clone()),
                translation_key: Some(name),
                show_on_tooltip: a.bool_or("ShowOnTooltip", true),
            },
        ));
    }
    Ok(UpdateAbilityKeywords {
        r#type: super::INIT,
        max_id: order.len() as i32,
        ability_keywords: Some(map),
    })
}
