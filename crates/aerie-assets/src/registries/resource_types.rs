//! `UpdateResourceTypes` (59) from `Server/Item/ResourceTypes`, keyed by name
//! (spec 07). `Icon` (inherited from `Parent`) → `icon`; the rest of the
//! asset stays on the server.

use aerie_protocol::proto::{ResourceType, UpdateResourceTypes};

use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub const PATH: &str = "Item/ResourceTypes";

pub fn build(ctx: &Ctx) -> Result<UpdateResourceTypes> {
    let store = ctx.store(PATH)?;
    let types = store
        .own
        .keys()
        .map(|k| {
            (
                k.clone(),
                ResourceType {
                    id: Some(k.clone()),
                    icon: store.inherited(k, &["Icon"]).str_at("Icon"),
                },
            )
        })
        .collect();
    Ok(UpdateResourceTypes {
        r#type: super::INIT,
        resource_types: Some(types),
    })
}
