//! `UpdateProjectileConfigs` (85) from `ProjectileConfigs`, keyed by name
//! (spec 07).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | `Physics` (`Type` `Standard` or `GroundFollow`) | `physics_config` | standard defaults below |
//! | `Model` (a model asset) | `model`, at unit scale | |
//! | `UseModelScale` | `min_scale`/`max_scale` from the model asset's `MinScale`/`MaxScale` (0.95/1.05) | 1/1 |
//! | `LaunchForce` | same | 1 |
//! | `SpawnOffset` `{X,Y,Z}` | same | 0,0,0 |
//! | `SpawnRotationOffset` `{Yaw,Pitch,Roll}` (degrees) | `rotation_offset` (radians) | 0,0,0 |
//! | `RotateSpawnOffsetByPitch`, `RotateSpawnOffsetByYaw` | same | true |
//! | `Interactions` (type → root) | root indexes | empty |
//! | `LaunchLocalSoundEventId`, `LaunchWorldSoundEventId`, `ProjectileSoundEventId` | sound event indexes | 0 |
//!
//! Standard physics: `Density` 700, `Gravity` 0, `Bounciness` 0,
//! `BounceCount` -1, `BounceLimit` 0.4, `SticksVertically` false,
//! `ComputeYaw`/`ComputePitch` true, `RotationMode` `VelocityDamped`,
//! `MoveOutOfSolidSpeed` 0, `TerminalVelocityAir` 1, `DensityAir` 1.2,
//! `TerminalVelocityWater` 1, `DensityWater` 998, `HitWaterImpulseLoss` 0.2,
//! `RotationForce` 3, `SpeedRotationFactor` 2, `SwimmingDampingFactor` 1,
//! `AllowRolling` false, `RollingFrictionFactor` 0.99, `RollingSpeed` 0.1.

use aerie_protocol::proto as p;
use serde_json::Value;

use super::music::{owner_of, reference};
use super::{models, root_interactions, sound_events};
use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const PATH: &str = "ProjectileConfigs";

fn physics(v: Option<&Value>) -> p::PhysicsConfig {
    let null = Value::Null;
    let v = v.unwrap_or(&null);
    p::PhysicsConfig {
        r#type: p::PhysicsType::default(),
        density: v.f64_or("Density", 700.0),
        gravity: v.f64_or("Gravity", 0.0),
        bounciness: v.f64_or("Bounciness", 0.0),
        bounce_count: v.i32_or("BounceCount", -1),
        bounce_limit: v.f64_or("BounceLimit", 0.4),
        sticks_vertically: v.bool_or("SticksVertically", false),
        compute_yaw: v.bool_or("ComputeYaw", true),
        compute_pitch: v.bool_or("ComputePitch", true),
        rotation_mode: enum_or(
            v,
            "RotationMode",
            p::RotationMode::from_name,
            p::RotationMode::from_name("VelocityDamped").unwrap_or_default(),
        ),
        move_out_of_solid_speed: v.f64_or("MoveOutOfSolidSpeed", 0.0),
        terminal_velocity_air: v.f64_or("TerminalVelocityAir", 1.0),
        density_air: v.f64_or("DensityAir", 1.2),
        terminal_velocity_water: v.f64_or("TerminalVelocityWater", 1.0),
        density_water: v.f64_or("DensityWater", 998.0),
        hit_water_impulse_loss: v.f64_or("HitWaterImpulseLoss", 0.2),
        rotation_force: v.f64_or("RotationForce", 3.0),
        speed_rotation_factor: v.f32_or("SpeedRotationFactor", 2.0),
        swimming_damping_factor: v.f64_or("SwimmingDampingFactor", 1.0),
        allow_rolling: v.bool_or("AllowRolling", false),
        rolling_friction_factor: v.f64_or("RollingFrictionFactor", 0.99),
        rolling_speed: v.f32_or("RollingSpeed", 0.1),
    }
}

/// `SpawnRotationOffset` after loading. A config that writes it takes each
/// missing field from its parent's value (already in radians), then
/// converts every field from degrees, so inherited fields are converted
/// twice; one that does not write it shares its parent's.
fn rotation(store: &crate::store::Store, k: &str, depth: usize) -> p::Direction {
    let parent = store
        .parents
        .get(k)
        .filter(|p| store.own.contains_key(*p) && depth < 32)
        .map(|p| rotation(store, p, depth + 1));
    let Some(own) = store.own.get(k).and_then(|o| o.obj("SpawnRotationOffset")) else {
        return parent.unwrap_or_default();
    };
    let base = parent.unwrap_or_default();
    let r = std::f32::consts::PI / 180.0;
    let f = |key: &str, inherited: f32| own.opt_f32(key).unwrap_or(inherited) * r;
    p::Direction {
        yaw: f("Yaw", base.yaw),
        pitch: f("Pitch", base.pitch),
        roll: f("Roll", base.roll),
    }
}

fn config(ctx: &Ctx, store: &crate::store::Store, k: &str) -> Result<p::ProjectileConfig> {
    let a = store.resolved(k, "ProjectileConfig");
    let model_key = a.str_at("Model");
    let model = match &model_key {
        Some(m) => models::model(ctx, m, None)?,
        None => None,
    };
    let (min_scale, max_scale) = if a.bool_or("UseModelScale", false) {
        let ms = ctx.store(models::MODELS)?;
        let asset = model_key
            .as_ref()
            .and_then(|m| ms.own.keys().find(|x| x.eq_ignore_ascii_case(m)).cloned())
            .map(|m| ms.resolved(&m, "ModelAsset"));
        match asset {
            Some(m) => (m.f32_or("MinScale", 0.95), m.f32_or("MaxScale", 1.05)),
            None => (1.0, 1.0),
        }
    } else {
        (1.0, 1.0)
    };
    let off = a.obj("SpawnOffset");
    let o = |key: &str| off.map_or(0.0, |v| v.f32_or(key, 0.0));
    let owner = owner_of(store, k, "Interactions");
    let mut interactions = Vec::new();
    for (t, v) in a
        .obj("Interactions")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let Some(ty) = p::InteractionType::from_name(t) else {
            continue;
        };
        let key = reference(v, &owner, &format!("Interactions_{t}"));
        interactions.push((
            ty,
            ctx.index_or(
                root_interactions::PATH,
                root_interactions::BUILTIN,
                key.as_deref(),
                i32::MIN,
            )?,
        ));
    }
    let sound = |key: &str| sound_events::index(ctx, a.str_at(key).as_deref());
    Ok(p::ProjectileConfig {
        physics_config: Some(physics(a.obj("Physics"))),
        model,
        launch_force: a.f64_or("LaunchForce", 1.0),
        spawn_offset: Some([o("X"), o("Y"), o("Z")]),
        rotation_offset: Some(rotation(store, k, 0)),
        rotate_spawn_offset_by_pitch: a.bool_or("RotateSpawnOffsetByPitch", true),
        rotate_spawn_offset_by_yaw: a.bool_or("RotateSpawnOffsetByYaw", true),
        interactions: Some(interactions),
        launch_local_sound_event_index: sound("LaunchLocalSoundEventId")?,
        launch_world_sound_event_index: sound("LaunchWorldSoundEventId")?,
        projectile_sound_event_index: sound("ProjectileSoundEventId")?,
        min_scale,
        max_scale,
    })
}

pub fn build(ctx: &Ctx) -> Result<p::UpdateProjectileConfigs> {
    let store = ctx.store(PATH)?;
    let mut list = Vec::new();
    for k in store.own.keys() {
        list.push((k.clone(), config(ctx, &store, k)?));
    }
    Ok(p::UpdateProjectileConfigs {
        r#type: super::INIT,
        configs: Some(list),
        removed_configs: None,
    })
}
