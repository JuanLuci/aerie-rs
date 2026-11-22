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

/// What the server needs to run stats (spec 17), as text lines for the
/// start-up cache: per stat `S index key initial min max shared reset
/// ignoreInvulnerability` (reset: 0 initial value, 1 max value), and per
/// `Regenerating` entry, in order, `R index interval amount percentage
/// clampAtZero <json>`, the JSON holding the entry's `Conditions` and
/// `Modifiers` as written; and `D stat value` for the gameplay config's
/// `Plugin.Stamina.SprintRegenDelay` (the stat set when a sprint ends).
pub fn table(ctx: &Ctx) -> Result<String> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let mut out = String::new();
    for (i, k) in order.iter().enumerate() {
        let nul = Value::Null;
        let a = store.assets.get(k).unwrap_or(&nul);
        out += &format!(
            "S {i} {k} {} {} {} {} {} {}\n",
            a.f64_or("InitialValue", 0.0),
            a.f64_or("Min", 0.0),
            a.f64_or("Max", 0.0),
            a.bool_or("Shared", false) as u8,
            (a.str_at("ResetType").as_deref() == Some("MaxValue")) as u8,
            a.bool_or("IgnoreInvulnerability", false) as u8,
        );
        for r in a
            .get("Regenerating")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let extra = serde_json::json!({
                "Conditions": r.get("Conditions").cloned().unwrap_or(Value::Null),
                "Modifiers": r.get("Modifiers").cloned().unwrap_or(Value::Null),
            });
            out += &format!(
                "R {i} {} {} {} {} {}\n",
                r.f64_or("Interval", 0.0),
                r.f64_or("Amount", 0.0),
                (r.str_at("RegenType").as_deref() == Some("Percentage")) as u8,
                r.bool_or("ClampAtZero", false) as u8,
                extra,
            );
        }
    }
    if let Ok(gameplay) = ctx.store("GameplayConfigs") {
        let d = gameplay.resolved("Default", "GameplayConfig");
        if let Some(s) = d
            .obj("Plugin")
            .and_then(|p| p.obj("Stamina"))
            .and_then(|s| s.obj("SprintRegenDelay"))
            && let Some(stat) = s.str_at("EntityStatId")
        {
            out += &format!("D {stat} {}\n", s.f64_or("Value", 0.0));
        }
    }
    Ok(out)
}
