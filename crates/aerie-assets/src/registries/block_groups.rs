//! `UpdateBlockGroups` (78) from `Server/Item/Groups`, keyed by name (spec 07).
//! `Blocks` (list of block names) → `names`.
//!
//! `UpdateBlockBreakingDecals` (45) from `Server/Item/Block/BreakingDecals`,
//! keyed by name. `StageTextures` (list) → `stage_textures`.

use aerie_protocol::proto::{
    BlockBreakingDecal, BlockGroup, UpdateBlockBreakingDecals, UpdateBlockGroups,
};

use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub const GROUPS: &str = "Item/Groups";
pub const DECALS: &str = "Item/Block/BreakingDecals";

pub fn build(ctx: &Ctx) -> Result<UpdateBlockGroups> {
    let store = ctx.store(GROUPS)?;
    let groups = store
        .assets
        .iter()
        .map(|(k, a)| {
            (
                k.clone(),
                BlockGroup {
                    names: a.strings("Blocks"),
                },
            )
        })
        .collect();
    Ok(UpdateBlockGroups {
        r#type: super::INIT,
        groups: Some(groups),
    })
}

pub fn build_decals(ctx: &Ctx) -> Result<UpdateBlockBreakingDecals> {
    let store = ctx.store(DECALS)?;
    let decals = store
        .assets
        .iter()
        .map(|(k, a)| {
            (
                k.clone(),
                BlockBreakingDecal {
                    stage_textures: a.strings("StageTextures").unwrap_or_default(),
                },
            )
        })
        .collect();
    Ok(UpdateBlockBreakingDecals {
        r#type: super::INIT,
        block_breaking_decals: decals,
    })
}
