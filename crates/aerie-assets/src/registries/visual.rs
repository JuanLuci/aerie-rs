//! `UpdateBlockParticleSets` (44) from `Server/Item/Block/Particles` (by
//! name) and `UpdateModelvfxs` (53) from `Server/Entity/ModelVFX` (indexed, no
//! built-in entries) (spec 07). Every field of both inherits from `Parent`.
//!
//! Block particle set: `Color`, `Scale` (1), `PositionOffset` `{X,Y,Z}`,
//! `RotationOffset` `{Yaw,Pitch,Roll}`, `Particles` (block particle event
//! name → particle system id).
//!
//! Model VFX: `SwitchTo` (Disappear), `EffectDirection` (None),
//! `AnimationDuration` (0), `AnimationRange` `{X,Y}` (0, 1), `LoopOption`
//! (PlayOnce), `CurveType` (Linear), `HighlightColor` (white),
//! `HighlightThickness` (0), `UseBloomOnHighlight`, `UseProgessiveHighlight`
//! (false; the key is spelled that way), `NoiseScale` `{X,Y}` (50, 50),
//! `NoiseScrollSpeed` `{X,Y}` (absent), `PostColor` (white),
//! `PostColorOpacity` (1), `Opacity` (1).

use aerie_protocol::proto::{
    BlockParticleEvent, BlockParticleSet, Color, CurveType, EffectDirection, LoopOption, ModelVFX,
    SwitchTo, UpdateBlockParticleSets, UpdateModelvfxs,
};
use serde_json::Value;

use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::{Get, direction, vec3};

pub const PARTICLE_SETS: &str = "Item/Block/Particles";
pub const MODEL_VFX: &str = "Entity/ModelVFX";

fn vec2(v: &Value) -> [f32; 2] {
    [v.f32_or("X", 0.0), v.f32_or("Y", 0.0)]
}

pub fn build_particle_sets(ctx: &Ctx) -> Result<UpdateBlockParticleSets> {
    let store = ctx.store(PARTICLE_SETS)?;
    let sets = store
        .assets
        .iter()
        .map(|(k, a)| {
            let particles = a.obj("Particles").and_then(Value::as_object).map(|m| {
                m.iter()
                    .filter_map(|(e, id)| {
                        Some((BlockParticleEvent::from_name(e)?, id.as_str()?.to_owned()))
                    })
                    .collect()
            });
            (
                k.clone(),
                BlockParticleSet {
                    id: Some(k.clone()),
                    color: a.color("Color"),
                    scale: a.f32_or("Scale", 1.0),
                    position_offset: a.obj("PositionOffset").map(vec3),
                    rotation_offset: a.obj("RotationOffset").map(direction),
                    particle_system_ids: particles,
                },
            )
        })
        .collect();
    Ok(UpdateBlockParticleSets {
        r#type: super::INIT,
        block_particle_sets: Some(sets),
    })
}

pub fn build_model_vfx(ctx: &Ctx) -> Result<UpdateModelvfxs> {
    let store = ctx.store(MODEL_VFX)?;
    let order = ctx.order(MODEL_VFX, &[])?;
    let white = Color {
        red: -1,
        green: -1,
        blue: -1,
    };
    let map = order
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let a = &store.assets[k];
            (
                i as i32,
                ModelVFX {
                    id: Some(k.clone()),
                    switch_to: enum_or(a, "SwitchTo", SwitchTo::from_name, SwitchTo::DISAPPEAR),
                    effect_direction: enum_or(
                        a,
                        "EffectDirection",
                        EffectDirection::from_name,
                        EffectDirection::NONE,
                    ),
                    animation_duration: a.f32_or("AnimationDuration", 0.0),
                    animation_range: Some(a.obj("AnimationRange").map(vec2).unwrap_or([0.0, 1.0])),
                    loop_option: enum_or(
                        a,
                        "LoopOption",
                        LoopOption::from_name,
                        LoopOption::PLAY_ONCE,
                    ),
                    curve_type: enum_or(a, "CurveType", CurveType::from_name, CurveType::LINEAR),
                    highlight_color: Some(a.color("HighlightColor").unwrap_or(white.clone())),
                    highlight_thickness: a.f32_or("HighlightThickness", 0.0),
                    use_bloom_on_highlight: a.bool_or("UseBloomOnHighlight", false),
                    use_progessive_highlight: a.bool_or("UseProgessiveHighlight", false),
                    noise_scale: Some(a.obj("NoiseScale").map(vec2).unwrap_or([50.0, 50.0])),
                    noise_scroll_speed: a.obj("NoiseScrollSpeed").map(vec2),
                    post_color: Some(a.color("PostColor").unwrap_or(white.clone())),
                    post_color_opacity: a.f32_or("PostColorOpacity", 1.0),
                    opacity: a.f32_or("Opacity", 1.0),
                },
            )
        })
        .collect();
    Ok(UpdateModelvfxs {
        r#type: super::INIT,
        max_id: order.len() as i32,
        model_vf_xs: Some(map),
    })
}
