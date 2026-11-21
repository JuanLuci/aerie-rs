//! `UpdateEntityStatTypes` (72) from `Server/Entity/Stats` (spec 07).
//!
//! Built-in entry at index 0: `Unknown` (all zero, reset to initial value).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | file name | `id` | |
//! | `InitialValue` | `value` | 0 |
//! | `Min`, `Max` | `min`, `max` | 0 |
//! | `MinValueEffects`, `MaxValueEffects` | same, see below | absent |
//! | `ResetType` (enum name) | `reset_behavior` | `InitialValue` |
//! | `HideFromTooltip` | `hide_from_tooltip` | `false` |
//!
//! Effects: `SoundEventId` → `sound_event_index` (index in SoundEvents, 0 when
//! absent), `TriggerAtZero` → `trigger_at_zero`, `Particles` → `particles`
//! (left out when empty).

use aerie_protocol::proto::{
    EntityStatEffects, EntityStatResetBehavior, EntityStatType, UpdateEntityStatTypes,
};
use serde_json::Value;

use super::{model_particles, sound_events};
use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const PATH: &str = "Entity/Stats";
pub const BUILTIN: &[&str] = &["Unknown"];

fn effects(ctx: &Ctx, v: &Value) -> Result<EntityStatEffects> {
    let particles = model_particles::list(ctx, v.get("Particles"))?;
    Ok(EntityStatEffects {
        trigger_at_zero: v.bool_or("TriggerAtZero", false),
        sound_event_index: sound_events::index(ctx, v.str_at("SoundEventId").as_deref())?,
        particles: (!particles.is_empty()).then_some(particles),
    })
}

fn entry(ctx: &Ctx, id: &str, a: &Value) -> Result<EntityStatType> {
    Ok(EntityStatType {
        id: Some(id.to_owned()),
        value: a.f32_or("InitialValue", 0.0),
        min: a.f32_or("Min", 0.0),
        max: a.f32_or("Max", 0.0),
        min_value_effects: a
            .obj("MinValueEffects")
            .map(|v| effects(ctx, v))
            .transpose()?,
        max_value_effects: a
            .obj("MaxValueEffects")
            .map(|v| effects(ctx, v))
            .transpose()?,
        reset_behavior: enum_or(
            a,
            "ResetType",
            EntityStatResetBehavior::from_name,
            EntityStatResetBehavior::INITIAL_VALUE,
        ),
        hide_from_tooltip: a.bool_or("HideFromTooltip", false),
    })
}

pub fn build(ctx: &Ctx) -> Result<UpdateEntityStatTypes> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let mut map = Vec::new();
    for (i, k) in order.iter().enumerate() {
        let e = if k == "Unknown" {
            EntityStatType {
                id: Some(k.clone()),
                reset_behavior: EntityStatResetBehavior::INITIAL_VALUE,
                ..Default::default()
            }
        } else {
            entry(ctx, k, &store.assets[k])?
        };
        map.push((i as i32, e));
    }
    Ok(UpdateEntityStatTypes {
        r#type: super::INIT,
        max_id: order.len() as i32,
        types: Some(map),
    })
}
