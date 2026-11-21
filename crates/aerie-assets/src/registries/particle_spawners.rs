//! `UpdateParticleSpawners` (50) from `Server/Particles/**.particlespawner`,
//! keyed by name (spec 07).
//!
//! Inheritance follows the schema (`Store::resolved`). Inherited from
//! `Parent`: `Shape` (Sphere), `EmitOffset`,
//! `UseEmitDirection`, `TotalParticles` `{Min, Max}`, `LifeSpan`,
//! `MaxConcurrentParticles`, `ParticleLifeSpan`, `SpawnRate`, `SpawnBurst`,
//! `WaveDelay`, `InitialVelocity`, `ParticleRotationInfluence` (None),
//! `ParticleRotateWithSpawner`, `TrailSpawnerPositionMultiplier`,
//! `TrailSpawnerRotationMultiplier`, `Attractors`, `RenderMode`
//! (BlendLinear), `LightInfluence`, `IntersectionHighlight`,
//! `LinearFiltering`, `UVMotion`, `ParticleCollision`, `IsLowRes`, and
//! `Particle` (inside it only `UseSpriteBlending` inherits). Not inherited:
//! `VelocityStretchMultiplier`, `CameraOffset`.
//!
//! Particle: `Texture`, `FrameSize` `{Width, Height}`, `UVOption` (None),
//! `ScaleRatioConstraint` (OneToOne), `SoftParticles` (Enable),
//! `SoftParticlesFadeFactor` (1), camera fade distances (0),
//! `UseSpriteBlending`, `InitialAnimationFrame`, `CollisionAnimationFrame`,
//! `Animation` (percent of lifetime → frame). Frame: `FrameIndex` `{Min,
//! Max}`, `Scale` `{X, Y}` and `Rotation` `{X, Y, Z}` (ranges), `Color`,
//! `Opacity` (-1). Collision: `BlockType` (None), `Action` (Expire),
//! `ParticleRotationInfluence` (falls back to the spawner value). UV motion:
//! `Texture`, `AddRandomUVOffset`, `SpeedX`, `SpeedY`, `Scale`, `Strength`,
//! `StrengthCurveType`.

use aerie_protocol::proto::{
    EmitShape, FXRenderMode, Particle, ParticleAnimationFrame, ParticleCollision,
    ParticleCollisionAction, ParticleCollisionBlockType, ParticleRotationInfluence,
    ParticleScaleRatioConstraint, ParticleSpawner, ParticleUVOption, Range, RangeVector2f, Size,
    SoftParticle, UVMotion, UVMotionCurveType, UpdateParticleSpawners,
};
use serde_json::Value;

use super::particles::{attractors, initial_velocity, range_vec3, rangef};
use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const SPAWNERS: &str = "Particles|.particlespawner";

fn range(v: &Value) -> Range {
    Range {
        min: v.i32_or("Min", 0),
        max: v.i32_or("Max", 0),
    }
}

fn frame(v: &Value) -> ParticleAnimationFrame {
    ParticleAnimationFrame {
        frame_index: v.obj("FrameIndex").map(range),
        scale: v.obj("Scale").map(|s| RangeVector2f {
            x: s.obj("X").map(rangef),
            y: s.obj("Y").map(rangef),
        }),
        rotation: v.obj("Rotation").map(range_vec3),
        color: v.color("Color"),
        opacity: v.f32_or("Opacity", -1.0),
    }
}

fn particle(v: &Value) -> Particle {
    Particle {
        texture_path: v.str_at("Texture"),
        frame_size: v.obj("FrameSize").map(|s| Size {
            width: s.i32_or("Width", 0),
            height: s.i32_or("Height", 0),
        }),
        uv_option: enum_or(
            v,
            "UVOption",
            ParticleUVOption::from_name,
            ParticleUVOption::NONE,
        ),
        scale_ratio_constraint: enum_or(
            v,
            "ScaleRatioConstraint",
            ParticleScaleRatioConstraint::from_name,
            ParticleScaleRatioConstraint::ONE_TO_ONE,
        ),
        soft_particles: enum_or(
            v,
            "SoftParticles",
            SoftParticle::from_name,
            SoftParticle::ENABLE,
        ),
        soft_particles_fade_factor: v.f32_or("SoftParticlesFadeFactor", 1.0),
        camera_near_fade_start_distance: v.f32_or("CameraNearFadeStartDistance", 0.0),
        camera_near_fade_end_distance: v.f32_or("CameraNearFadeEndDistance", 0.0),
        camera_far_fade_start_distance: v.f32_or("CameraFarFadeStartDistance", 0.0),
        camera_far_fade_end_distance: v.f32_or("CameraFarFadeEndDistance", 0.0),
        use_sprite_blending: v.bool_or("UseSpriteBlending", false),
        initial_animation_frame: v.obj("InitialAnimationFrame").map(frame),
        collision_animation_frame: v.obj("CollisionAnimationFrame").map(frame),
        animation_frames: v.obj("Animation").and_then(Value::as_object).map(|m| {
            m.iter()
                .filter_map(|(k, f)| Some((k.parse::<i32>().ok()?, frame(f))))
                .collect()
        }),
    }
}

fn spawner(id: &str, a: &Value, particle_json: Option<Value>) -> ParticleSpawner {
    let influence = enum_or(
        a,
        "ParticleRotationInfluence",
        ParticleRotationInfluence::from_name,
        ParticleRotationInfluence::NONE,
    );
    ParticleSpawner {
        id: Some(id.to_owned()),
        particle: particle_json.as_ref().map(particle),
        shape: enum_or(a, "Shape", EmitShape::from_name, EmitShape::SPHERE),
        emit_offset: a.obj("EmitOffset").map(range_vec3),
        camera_offset: a.f32_or("CameraOffset", 0.0),
        use_emit_direction: a.bool_or("UseEmitDirection", false),
        life_span: a.f32_or("LifeSpan", 0.0),
        spawn_rate: a.obj("SpawnRate").map(rangef),
        spawn_burst: a.bool_or("SpawnBurst", false),
        wave_delay: a.obj("WaveDelay").map(rangef),
        total_particles: a.obj("TotalParticles").map(range),
        max_concurrent_particles: a.i32_or("MaxConcurrentParticles", 0),
        initial_velocity: a.obj("InitialVelocity").map(initial_velocity),
        velocity_stretch_multiplier: a.f32_or("VelocityStretchMultiplier", 0.0),
        particle_rotation_influence: influence,
        particle_rotate_with_spawner: a.bool_or("ParticleRotateWithSpawner", false),
        is_low_res: a.bool_or("IsLowRes", false),
        trail_spawner_position_multiplier: a.f32_or("TrailSpawnerPositionMultiplier", 0.0),
        trail_spawner_rotation_multiplier: a.f32_or("TrailSpawnerRotationMultiplier", 0.0),
        particle_collision: a.obj("ParticleCollision").map(|c| ParticleCollision {
            block_type: enum_or(
                c,
                "BlockType",
                ParticleCollisionBlockType::from_name,
                ParticleCollisionBlockType::NONE,
            ),
            action: enum_or(
                c,
                "Action",
                ParticleCollisionAction::from_name,
                ParticleCollisionAction::EXPIRE,
            ),
            particle_rotation_influence: enum_or(
                c,
                "ParticleRotationInfluence",
                ParticleRotationInfluence::from_name,
                influence,
            ),
        }),
        render_mode: enum_or(
            a,
            "RenderMode",
            FXRenderMode::from_name,
            FXRenderMode::BLEND_LINEAR,
        ),
        light_influence: a.f32_or("LightInfluence", 0.0),
        linear_filtering: a.bool_or("LinearFiltering", false),
        particle_life_span: a.obj("ParticleLifeSpan").map(rangef),
        uv_motion: a.obj("UVMotion").map(|u| UVMotion {
            texture: u.str_at("Texture"),
            add_random_uv_offset: u.bool_or("AddRandomUVOffset", false),
            speed_x: u.f32_or("SpeedX", 0.0),
            speed_y: u.f32_or("SpeedY", 0.0),
            scale: u.f32_or("Scale", 0.0),
            strength: u.f32_or("Strength", 0.0),
            strength_curve_type: enum_or(
                u,
                "StrengthCurveType",
                UVMotionCurveType::from_name,
                UVMotionCurveType::default(),
            ),
        }),
        attractors: attractors(a),
        intersection_highlight: a.obj("IntersectionHighlight").map(super::trails::highlight),
    }
}

pub fn build(ctx: &Ctx) -> Result<UpdateParticleSpawners> {
    let store = ctx.store(SPAWNERS)?;
    let spawners = store
        .own
        .keys()
        .map(|k| {
            let a = store.resolved(k, "ParticleSpawner");
            let particle = a.get("Particle").cloned();
            (k.clone(), spawner(k, &a, particle))
        })
        .collect();
    Ok(UpdateParticleSpawners {
        r#type: super::INIT,
        particle_spawners: Some(spawners),
        removed_particle_spawners: None,
    })
}
