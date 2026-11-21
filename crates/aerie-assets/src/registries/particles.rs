//! `UpdateParticleSystems` (49) from `Server/Particles/**.particlesystem`, keyed by name (spec 07).
//! Every system field inherits from `Parent`.
//!
//! System: `Spawners` (groups, see below), `LifeSpan`, `CullDistance`,
//! `BoundingRadius` (0), `IsImportant` (false).
//! Spawner group: `SpawnerId`, `PositionOffset` `{X,Y,Z}`, `RotationOffset`
//! `{Yaw,Pitch,Roll}`, `FixedRotation`, `SpawnRate`, `LifeSpan`, `WaveDelay`
//! (ranges `{Min, Max}`), `StartDelay` (0), `TotalSpawners` (1),
//! `MaxConcurrent` (0), `InitialVelocity` `{Yaw, Pitch, Speed}` (ranges),
//! `EmitOffset` `{X, Y, Z}` (ranges), `Attractors`.
//! Attractor: `Position`, `RadialAxis`, `LinearAcceleration`, `LinearImpulse`,
//! `DampingMultiplier` (vectors, absent by default), `TrailPositionMultiplier`,
//! `Radius`, `RadialAcceleration`, `RadialTangentAcceleration`,
//! `RadialImpulse`, `RadialTangentImpulse` (0). Empty lists are left out.

use aerie_protocol::proto::{
    InitialVelocity, ParticleAttractor, ParticleSpawnerGroup, ParticleSystem, RangeVector3f,
    Rangef, UpdateParticleSystems,
};
use serde_json::Value;

use crate::values::{Get, direction, vec3};
use crate::{Result, ctx::Ctx};

pub const SYSTEMS: &str = "Particles|.particlesystem";

pub fn rangef(v: &Value) -> Rangef {
    Rangef {
        min: v.f32_or("Min", 0.0),
        max: v.f32_or("Max", 0.0),
    }
}

pub fn range_vec3(v: &Value) -> RangeVector3f {
    RangeVector3f {
        x: v.obj("X").map(rangef),
        y: v.obj("Y").map(rangef),
        z: v.obj("Z").map(rangef),
    }
}

pub fn initial_velocity(v: &Value) -> InitialVelocity {
    InitialVelocity {
        yaw: v.obj("Yaw").map(rangef),
        pitch: v.obj("Pitch").map(rangef),
        speed: v.obj("Speed").map(rangef),
    }
}

pub fn attractor(v: &Value) -> ParticleAttractor {
    ParticleAttractor {
        position: v.obj("Position").map(vec3),
        radial_axis: v.obj("RadialAxis").map(vec3),
        trail_position_multiplier: v.f32_or("TrailPositionMultiplier", 0.0),
        radius: v.f32_or("Radius", 0.0),
        radial_acceleration: v.f32_or("RadialAcceleration", 0.0),
        radial_tangent_acceleration: v.f32_or("RadialTangentAcceleration", 0.0),
        linear_acceleration: v.obj("LinearAcceleration").map(vec3),
        radial_impulse: v.f32_or("RadialImpulse", 0.0),
        radial_tangent_impulse: v.f32_or("RadialTangentImpulse", 0.0),
        linear_impulse: v.obj("LinearImpulse").map(vec3),
        damping_multiplier: v.obj("DampingMultiplier").map(vec3),
    }
}

pub fn attractors(v: &Value) -> Option<Vec<ParticleAttractor>> {
    let a: Vec<_> = v
        .get("Attractors")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(attractor).collect())
        .unwrap_or_default();
    (!a.is_empty()).then_some(a)
}

fn group(v: &Value) -> ParticleSpawnerGroup {
    ParticleSpawnerGroup {
        spawner_id: v.str_at("SpawnerId"),
        position_offset: v.obj("PositionOffset").map(vec3),
        rotation_offset: v.obj("RotationOffset").map(direction),
        fixed_rotation: v.bool_or("FixedRotation", false),
        start_delay: v.f32_or("StartDelay", 0.0),
        spawn_rate: v.obj("SpawnRate").map(rangef),
        wave_delay: v.obj("WaveDelay").map(rangef),
        total_spawners: v.i32_or("TotalSpawners", 1),
        max_concurrent: v.i32_or("MaxConcurrent", 0),
        initial_velocity: v.obj("InitialVelocity").map(initial_velocity),
        emit_offset: v.obj("EmitOffset").map(range_vec3),
        life_span: v.obj("LifeSpan").map(rangef),
        attractors: attractors(v),
    }
}

pub fn build_systems(ctx: &Ctx) -> Result<UpdateParticleSystems> {
    let store = ctx.store(SYSTEMS)?;
    let systems = store
        .assets
        .iter()
        .map(|(k, a)| {
            let spawners: Vec<_> = a
                .get("Spawners")
                .and_then(Value::as_array)
                .map(|s| s.iter().map(group).collect())
                .unwrap_or_default();
            (
                k.clone(),
                ParticleSystem {
                    id: Some(k.clone()),
                    spawners: (!spawners.is_empty()).then_some(spawners),
                    life_span: a.f32_or("LifeSpan", 0.0),
                    cull_distance: a.f32_or("CullDistance", 0.0),
                    bounding_radius: a.f32_or("BoundingRadius", 0.0),
                    is_important: a.bool_or("IsImportant", false),
                },
            )
        })
        .collect();
    Ok(UpdateParticleSystems {
        r#type: super::INIT,
        particle_systems: Some(systems),
        removed_particle_systems: None,
    })
}
