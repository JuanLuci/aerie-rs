//! Sound sets: maps from an event to a sound event, sent as sound event
//! indexes (spec 07). An unknown sound event becomes `i32::MIN`.
//!
//! - `UpdateItemSoundSets` (43) from `Server/Audio/ItemSounds`: `SoundEvents`
//!   (item sound event name → sound event).
//! - `UpdateBlockSoundSets` (42) from `Server/Item/Block/Sounds`, built-in
//!   `EMPTY` at index 0: `SoundEvents` (block sound event name → sound event),
//!   `MoveInRepeatRange` `[min, max]` (default `[0.5, 1.5]`).
//! - `UpdateSoundSets` (79) from `Server/Audio/SoundSets`, built-in `EMPTY` at
//!   index 0: `SoundEvents` (any name → sound event), `Category` (SFX).
//!
//! Sets without sound events still send an empty map.

use aerie_protocol::proto::{
    BlockSoundEvent, BlockSoundSet, FloatRange, ItemSoundEvent, ItemSoundSet, SoundCategory,
    SoundSet, UpdateBlockSoundSets, UpdateItemSoundSets, UpdateSoundSets,
};
use serde_json::Value;

use super::sound_events;
use crate::Result;
use crate::ctx::{Ctx, enum_or};

pub const ITEM: &str = "Audio/ItemSounds";
pub const BLOCK: &str = "Item/Block/Sounds";
pub const SET: &str = "Audio/SoundSets";
pub const EMPTY: &[&str] = &["EMPTY"];

/// `SoundEvents` as (key, sound event index) pairs, keys converted by `key`.
fn events<K>(
    ctx: &Ctx,
    a: Option<&Value>,
    key: impl Fn(&str) -> Option<K>,
) -> Result<Vec<(K, i32)>> {
    let mut out = Vec::new();
    if let Some(m) = a
        .and_then(|a| a.get("SoundEvents"))
        .and_then(Value::as_object)
    {
        for (k, v) in m {
            let Some(k) = key(k) else { continue };
            out.push((k, sound_events::index(ctx, v.as_str())?));
        }
    }
    Ok(out)
}

pub fn build_item(ctx: &Ctx) -> Result<UpdateItemSoundSets> {
    let store = ctx.store(ITEM)?;
    let order = ctx.order(ITEM, &[])?;
    let mut map = Vec::new();
    for (i, k) in order.iter().enumerate() {
        let ev = events(ctx, store.assets.get(k), ItemSoundEvent::from_name)?;
        map.push((
            i as i32,
            ItemSoundSet {
                id: Some(k.clone()),
                sound_event_indices: Some(ev),
            },
        ));
    }
    Ok(UpdateItemSoundSets {
        r#type: super::INIT,
        max_id: order.len() as i32,
        item_sound_sets: Some(map),
    })
}

pub fn build_block(ctx: &Ctx) -> Result<UpdateBlockSoundSets> {
    let store = ctx.store(BLOCK)?;
    let order = ctx.order(BLOCK, EMPTY)?;
    let mut map = Vec::new();
    for (i, k) in order.iter().enumerate() {
        let a = store.assets.get(k);
        let range = a
            .and_then(|a| a.get("MoveInRepeatRange"))
            .and_then(Value::as_array);
        let (lo, hi) = match range {
            Some(r) if r.len() == 2 => (
                r[0].as_f64().unwrap_or(0.5) as f32,
                r[1].as_f64().unwrap_or(1.5) as f32,
            ),
            _ => (0.5, 1.5),
        };
        map.push((
            i as i32,
            BlockSoundSet {
                id: Some(k.clone()),
                sound_event_indices: Some(events(ctx, a, BlockSoundEvent::from_name)?),
                move_in_repeat_range: Some(FloatRange {
                    inclusive_min: lo,
                    inclusive_max: hi,
                }),
            },
        ));
    }
    Ok(UpdateBlockSoundSets {
        r#type: super::INIT,
        max_id: order.len() as i32,
        block_sound_sets: Some(map),
    })
}

pub fn build_set(ctx: &Ctx) -> Result<UpdateSoundSets> {
    let store = ctx.store(SET)?;
    let order = ctx.order(SET, EMPTY)?;
    let mut map = Vec::new();
    for (i, k) in order.iter().enumerate() {
        let a = store.assets.get(k);
        map.push((
            i as i32,
            SoundSet {
                id: Some(k.clone()),
                sounds: Some(events(ctx, a, |s| Some(s.to_owned()))?),
                category: a
                    .map(|a| enum_or(a, "Category", SoundCategory::from_name, SoundCategory::SFX))
                    .unwrap_or(SoundCategory::SFX),
            },
        ));
    }
    Ok(UpdateSoundSets {
        r#type: super::INIT,
        max_id: order.len() as i32,
        sound_sets: Some(map),
    })
}
