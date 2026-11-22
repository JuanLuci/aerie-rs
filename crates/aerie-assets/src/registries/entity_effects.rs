//! `UpdateEntityEffects` (51) from `Entity/Effects`, indexed, no built-in
//! entries; 35 more are contained in interactions and NPC roles (spec 07).
//! Inheritance follows the schema (`Store::resolved`): every field inherits,
//! and inside `ApplicationEffects` (with its `MovementEffects` and
//! `AbilityEffects`) and `ModelOverride` every field does too.
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | key | `id` | |
//! | `Name` | `name` | absent |
//! | `ApplicationEffects` | `application_effects` | absent |
//! | `WorldRemovalSoundEventId` | `world_removal_sound_event_index` | 0 |
//! | `LocalRemovalSoundEventId` | `local_removal_sound_event_index` | the world one |
//! | `ModelOverride` | `model_override` | absent |
//! | `Duration` | `duration` | 0 |
//! | `Infinite`, `Debuff` | same | `false` |
//! | `StatusEffectIcon` | `status_effect_icon` | absent |
//! | `OverlapBehavior` | `overlap_behavior` | `Ignore` |
//! | `DamageCalculatorCooldown` | `damage_calculator_cooldown` | 0 |
//! | `StatModifiers` `{stat: amount}` | `stat_modifiers` (stat index → amount; unknown stats dropped) | absent |
//! | `ValueType` | `value_type` | `Absolute` |
//!
//! Application effects: `EntityBottomTint`, `EntityTopTint` (colors),
//! `EntityAnimationId`, `Particles` and `FirstPersonParticles` (model
//! particles; empty lists are absent), `ScreenEffect`,
//! `HorizontalSpeedMultiplier` (1), `LocalSoundEventId` / `WorldSoundEventId`
//! (indexes; local falls back to world), `ModelVFXId`, `MovementEffects`
//! (`Disable{Forward,Backward,Left,Right,Sprint,Jump,Crouch}`, all set by
//! `DisableAll`,
//! `SpeedMultiplier` (1)), `AbilityEffects` (`Disabled`: interaction types),
//! `MouseSensitivityAdjustmentTarget` / `Duration` (0). Model override:
//! `Model`, `Texture`, `AnimationSets` (absent when empty).

use aerie_protocol::proto::{
    AbilityEffects, ApplicationEffects, EntityEffect, InteractionType, ModelOverride,
    MovementEffects, OverlapBehavior, UpdateEntityEffects, ValueType,
};
use serde_json::Value;

use super::{entity_stat_types, model_particles, models, sound_events};
use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const PATH: &str = "Entity/Effects";

fn particles(v: &Value, key: &str) -> Option<Vec<aerie_protocol::proto::ModelParticle>> {
    let a = v.get(key)?.as_array()?;
    (!a.is_empty()).then(|| a.iter().map(model_particles::one).collect())
}

/// Movement effects (shared with interaction effects).
pub fn movement(m: &Value) -> MovementEffects {
    let all = m.bool_or("DisableAll", false);
    let d = |k: &str| all || m.bool_or(k, false);
    MovementEffects {
        disable_forward: d("DisableForward"),
        disable_backward: d("DisableBackward"),
        disable_left: d("DisableLeft"),
        disable_right: d("DisableRight"),
        disable_sprint: d("DisableSprint"),
        disable_jump: d("DisableJump"),
        disable_crouch: d("DisableCrouch"),
        speed_multiplier: m.f32_or("SpeedMultiplier", 1.0),
    }
}

fn application(ctx: &Ctx, v: &Value) -> Result<ApplicationEffects> {
    let world = sound_events::index(ctx, v.str_at("WorldSoundEventId").as_deref())?;
    let local = sound_events::index(ctx, v.str_at("LocalSoundEventId").as_deref())?;
    Ok(ApplicationEffects {
        entity_bottom_tint: v.color("EntityBottomTint"),
        entity_top_tint: v.color("EntityTopTint"),
        entity_animation_id: v.str_at("EntityAnimationId"),
        particles: particles(v, "Particles"),
        first_person_particles: particles(v, "FirstPersonParticles"),
        screen_effect: v.str_at("ScreenEffect"),
        horizontal_speed_multiplier: v.f32_or("HorizontalSpeedMultiplier", 1.0),
        sound_event_index_local: if local != 0 { local } else { world },
        sound_event_index_world: world,
        model_vfx_id: v.str_at("ModelVFXId"),
        movement_effects: v.obj("MovementEffects").map(movement),
        mouse_sensitivity_adjustment_target: v.f32_or("MouseSensitivityAdjustmentTarget", 0.0),
        mouse_sensitivity_adjustment_duration: v.f32_or("MouseSensitivityAdjustmentDuration", 0.0),
        ability_effects: v.obj("AbilityEffects").map(|a| AbilityEffects {
            disabled: a.get("Disabled").and_then(Value::as_array).map(|l| {
                l.iter()
                    .filter_map(|t| t.as_str().and_then(InteractionType::from_name))
                    .collect()
            }),
        }),
    })
}

fn model_override(ctx: &Ctx, v: &Value) -> Result<ModelOverride> {
    let sets = match v.obj("AnimationSets").and_then(Value::as_object) {
        Some(m) if !m.is_empty() => Some(
            m.iter()
                .map(|(k, s)| Ok((k.clone(), models::animation_set(ctx, k, s)?)))
                .collect::<Result<Vec<_>>>()?,
        ),
        _ => None,
    };
    Ok(ModelOverride {
        model: v.str_at("Model"),
        texture: v.str_at("Texture"),
        animation_sets: sets,
    })
}

pub fn effect(ctx: &Ctx, id: &str, a: &Value) -> Result<EntityEffect> {
    let world = sound_events::index(ctx, a.str_at("WorldRemovalSoundEventId").as_deref())?;
    let local = sound_events::index(ctx, a.str_at("LocalRemovalSoundEventId").as_deref())?;
    let stats = match a.obj("StatModifiers").and_then(Value::as_object) {
        Some(m) => {
            let mut out = Vec::new();
            for (stat, amount) in m {
                let i = ctx.index_or(
                    entity_stat_types::PATH,
                    entity_stat_types::BUILTIN,
                    Some(stat),
                    i32::MIN,
                )?;
                if i != i32::MIN {
                    out.push((i, amount.as_f64().unwrap_or(0.0) as f32));
                }
            }
            (!out.is_empty()).then_some(out)
        }
        None => None,
    };
    Ok(EntityEffect {
        id: Some(id.to_owned()),
        name: a.str_at("Name"),
        application_effects: a
            .obj("ApplicationEffects")
            .map(|v| application(ctx, v))
            .transpose()?,
        world_removal_sound_event_index: world,
        local_removal_sound_event_index: if local != 0 { local } else { world },
        model_override: a
            .obj("ModelOverride")
            .map(|v| model_override(ctx, v))
            .transpose()?,
        duration: a.f32_or("Duration", 0.0),
        infinite: a.bool_or("Infinite", false),
        debuff: a.bool_or("Debuff", false),
        status_effect_icon: a.str_at("StatusEffectIcon"),
        overlap_behavior: enum_or(
            a,
            "OverlapBehavior",
            OverlapBehavior::from_name,
            OverlapBehavior::IGNORE,
        ),
        damage_calculator_cooldown: a.f32_or("DamageCalculatorCooldown", 0.0) as f64,
        stat_modifiers: stats,
        value_type: enum_or(a, "ValueType", ValueType::from_name, ValueType::ABSOLUTE),
    })
}

/// What the server needs of an effect and the packet does not carry: one
/// line per effect, `index<TAB>{RemovalBehavior, Invulnerable,
/// RawStatModifiers: [[stat, multiplicative, amount]], ApplyConditions}`.
pub fn server_table(ctx: &Ctx) -> Result<String> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, &[])?;
    let idx = ctx.indexes(PATH, &[])?;
    let mut out = String::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        let a = store.resolved(k, "EntityEffect");
        let mut o = serde_json::Map::new();
        for f in ["RemovalBehavior", "Invulnerable", "ApplyConditions"] {
            if let Some(v) = a.get(f) {
                o.insert(f.to_owned(), v.clone());
            }
        }
        let mut raw = Vec::new();
        for (stat, list) in a
            .obj("RawStatModifiers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let s = ctx.index_or(
                entity_stat_types::PATH,
                entity_stat_types::BUILTIN,
                Some(stat),
                i32::MIN,
            )?;
            if s == i32::MIN {
                continue;
            }
            for m in list.as_array().into_iter().flatten() {
                let mult = m
                    .str_at("CalculationType")
                    .is_some_and(|c| c.eq_ignore_ascii_case("Multiplicative"));
                raw.push(serde_json::json!([s, mult, m.f32_or("Amount", 0.0)]));
            }
        }
        if !raw.is_empty() {
            o.insert("RawStatModifiers".to_owned(), Value::Array(raw));
        }
        if !o.is_empty() {
            out += &format!("{i}\t{}\n", Value::Object(o));
        }
    }
    Ok(out)
}

pub fn build(ctx: &Ctx) -> Result<UpdateEntityEffects> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, &[])?;
    let idx = ctx.indexes(PATH, &[])?;
    let mut list = Vec::new();
    for k in &order {
        let i = idx
            .get(&k.to_lowercase())
            .copied()
            .unwrap_or(list.len() as i32);
        list.push((i, effect(ctx, k, &store.resolved(k, "EntityEffect"))?));
    }
    Ok(UpdateEntityEffects {
        r#type: super::INIT,
        max_id: order.len() as i32,
        entity_effects: Some(list),
    })
}
