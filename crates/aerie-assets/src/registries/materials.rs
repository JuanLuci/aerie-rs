//! `UpdatePhysicalMaterials` (87) from `Server/Item/Block/PhysicalMaterials`
//! and `UpdateFluidFX` (63) from `Server/Item/Block/FluidFX` (spec 07).
//! Every field of both inherits from `Parent`.
//!
//! Physical material (built-in `EMPTY`): `ReflectionCoeff`,
//! `AttenuationPerBlock`, `HFAttenuationPerBlock`, `ShelterOpacity` (all 0).
//!
//! Fluid effect (built-in `Empty`): `Fog` (enum, Color), `FogColor` (white),
//! `FogDistance` `[near, far]` (0, 32), `FogDepthStart` (40),
//! `FogDepthFalloff` (10), `ColorsFilter` `[r, g, b]` in 0..1 (each × 255,
//! truncated; white), `ColorsSaturation` (1), `DistortionAmplitude` (8),
//! `DistortionFrequency` (4), `Particle` `{SystemId, Color, Scale (1)}`,
//! `MovementSettings` `{SwimUpSpeed, SwimDownSpeed, SinkSpeed,
//! HorizontalSpeedMultiplier, FieldOfViewMultiplier, EntryVelocityMultiplier}`.

use aerie_protocol::proto::{
    Color, FluidFX, FluidFXMovementSettings, FluidFog, FluidParticle, NearFar, PhysicalMaterial,
    UpdateFluidFX, UpdatePhysicalMaterials,
};
use serde_json::Value;

use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const MATERIALS: &str = "Item/Block/PhysicalMaterials";
pub const MATERIALS_BUILTIN: &[&str] = &["EMPTY"];
pub const FLUID_FX: &str = "Item/Block/FluidFX";
pub const FLUID_FX_BUILTIN: &[&str] = &["Empty"];

static NULL: Value = Value::Null;

pub fn build_materials(ctx: &Ctx) -> Result<UpdatePhysicalMaterials> {
    let store = ctx.store(MATERIALS)?;
    let order = ctx.order(MATERIALS, MATERIALS_BUILTIN)?;
    let map = order
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let a = store.assets.get(k).unwrap_or(&NULL);
            (
                i as i32,
                PhysicalMaterial {
                    id: Some(k.clone()),
                    reflection_coeff: a.f32_or("ReflectionCoeff", 0.0),
                    attenuation_per_block: a.f32_or("AttenuationPerBlock", 0.0),
                    h_f_attenuation_per_block: a.f32_or("HFAttenuationPerBlock", 0.0),
                    shelter_opacity: a.f32_or("ShelterOpacity", 0.0),
                },
            )
        })
        .collect();
    Ok(UpdatePhysicalMaterials {
        r#type: super::INIT,
        max_id: order.len() as i32,
        physical_materials: Some(map),
    })
}

fn floats(v: &Value, key: &str) -> Option<Vec<f32>> {
    v.get(key)?.as_array().map(|a| {
        a.iter()
            .filter_map(Value::as_f64)
            .map(|f| f as f32)
            .collect()
    })
}

fn fluid_fx(id: &str, a: &Value) -> FluidFX {
    let fog = floats(a, "FogDistance")
        .filter(|f| f.len() >= 2)
        .unwrap_or(vec![0.0, 32.0]);
    let filter = floats(a, "ColorsFilter")
        .filter(|f| f.len() >= 3)
        .unwrap_or(vec![1.0, 1.0, 1.0]);
    let byte = |f: f32| (f * 255.0) as i32 as i8;
    let white = Color {
        red: -1,
        green: -1,
        blue: -1,
    };
    FluidFX {
        id: Some(id.to_owned()),
        shader: Default::default(),
        fog_mode: enum_or(a, "Fog", FluidFog::from_name, FluidFog::COLOR),
        fog_color: Some(a.color("FogColor").unwrap_or(white)),
        fog_distance: Some(NearFar {
            near: fog[0],
            far: fog[1],
        }),
        fog_depth_start: a.f32_or("FogDepthStart", 40.0),
        fog_depth_falloff: a.f32_or("FogDepthFalloff", 10.0),
        color_filter: Some(Color {
            red: byte(filter[0]),
            green: byte(filter[1]),
            blue: byte(filter[2]),
        }),
        color_saturation: a.f32_or("ColorsSaturation", 1.0),
        distortion_amplitude: a.f32_or("DistortionAmplitude", 8.0),
        distortion_frequency: a.f32_or("DistortionFrequency", 4.0),
        particle: a.obj("Particle").map(|p| FluidParticle {
            system_id: p.str_at("SystemId"),
            color: p.color("Color"),
            scale: p.f32_or("Scale", 1.0),
        }),
        movement_settings: a.obj("MovementSettings").map(|m| FluidFXMovementSettings {
            swim_up_speed: m.f32_or("SwimUpSpeed", 0.0),
            swim_down_speed: m.f32_or("SwimDownSpeed", 0.0),
            sink_speed: m.f32_or("SinkSpeed", 0.0),
            horizontal_speed_multiplier: m.f32_or("HorizontalSpeedMultiplier", 0.0),
            field_of_view_multiplier: m.f32_or("FieldOfViewMultiplier", 0.0),
            entry_velocity_multiplier: m.f32_or("EntryVelocityMultiplier", 0.0),
        }),
    }
}

pub fn build_fluid_fx(ctx: &Ctx) -> Result<UpdateFluidFX> {
    let store = ctx.store(FLUID_FX)?;
    let order = ctx.order(FLUID_FX, FLUID_FX_BUILTIN)?;
    let map = order
        .iter()
        .enumerate()
        .map(|(i, k)| (i as i32, fluid_fx(k, store.assets.get(k).unwrap_or(&NULL))))
        .collect();
    Ok(UpdateFluidFX {
        r#type: super::INIT,
        max_id: order.len() as i32,
        fluid_fx: Some(map),
    })
}
