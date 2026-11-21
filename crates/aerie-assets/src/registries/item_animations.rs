//! `UpdateItemPlayerAnimations` (52) from `Server/Item/Animations`, keyed by
//! name (spec 07).
//!
//! Inherited from `Parent`: `Animations` (combined by entry: the child's
//! entries replace the parent's with the same name), `WiggleWeights`,
//! `Camera`, `PullbackConfig`, `UseFirstPersonOverrides` (false).
//!
//! Animation: `ThirdPerson`, `ThirdPersonMoving`, `ThirdPersonFace`,
//! `FirstPerson`, `FirstPersonOverride`, `KeepPreviousFirstPersonAnimation`,
//! `Speed` (0), `BlendingDuration` (0.2), `Looping`, `ClipsGeometry`.
//! Wiggle weights: `X`, `Y`, `Z`, `Roll`, `Pitch` and each `…Deceleration` (0).
//! Camera: `PositionOffset` `{X,Y,Z}`, `Yaw`/`Pitch` `{AngleRange {Min,Max},
//! TargetNodes}`. Pullback: `Left`/`Right` `OffsetOverride`/`RotationOverride`
//! as `[x, y, z]` arrays.

use aerie_protocol::proto::{
    CameraAxis, CameraNode, CameraSettings, ItemAnimation, ItemPlayerAnimations,
    ItemPullbackConfiguration, UpdateItemPlayerAnimations, WiggleWeights,
};
use serde_json::Value;

use super::particles::rangef;
use crate::values::{Get, vec3};
use crate::{Result, ctx::Ctx};

pub const PATH: &str = "Item/Animations";
const INHERITED: &[&str] = &[
    "WiggleWeights",
    "Camera",
    "PullbackConfig",
    "UseFirstPersonOverrides",
];

fn animation(v: &Value) -> ItemAnimation {
    ItemAnimation {
        third_person: v.str_at("ThirdPerson"),
        third_person_moving: v.str_at("ThirdPersonMoving"),
        third_person_face: v.str_at("ThirdPersonFace"),
        first_person: v.str_at("FirstPerson"),
        first_person_override: v.str_at("FirstPersonOverride"),
        keep_previous_first_person_animation: v.bool_or("KeepPreviousFirstPersonAnimation", false),
        speed: v.f32_or("Speed", 0.0),
        blending_duration: v.f32_or("BlendingDuration", 0.2),
        looping: v.bool_or("Looping", false),
        clips_geometry: v.bool_or("ClipsGeometry", false),
    }
}

fn wiggle(v: &Value) -> WiggleWeights {
    WiggleWeights {
        x: v.f32_or("X", 0.0),
        x_deceleration: v.f32_or("XDeceleration", 0.0),
        y: v.f32_or("Y", 0.0),
        y_deceleration: v.f32_or("YDeceleration", 0.0),
        z: v.f32_or("Z", 0.0),
        z_deceleration: v.f32_or("ZDeceleration", 0.0),
        roll: v.f32_or("Roll", 0.0),
        roll_deceleration: v.f32_or("RollDeceleration", 0.0),
        pitch: v.f32_or("Pitch", 0.0),
        pitch_deceleration: v.f32_or("PitchDeceleration", 0.0),
    }
}

fn axis(v: &Value) -> CameraAxis {
    CameraAxis {
        angle_range: v.obj("AngleRange").map(rangef),
        target_nodes: v.get("TargetNodes").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(|n| n.as_str().and_then(CameraNode::from_name))
                .collect()
        }),
    }
}

pub fn camera(v: &Value) -> CameraSettings {
    CameraSettings {
        position_offset: v.obj("PositionOffset").map(vec3),
        yaw: v.obj("Yaw").map(axis),
        pitch: v.obj("Pitch").map(axis),
    }
}

fn arr3(v: &Value, key: &str) -> Option<[f32; 3]> {
    let a = v.get(key)?.as_array()?;
    Some([
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    ])
}

fn pullback(v: &Value) -> ItemPullbackConfiguration {
    ItemPullbackConfiguration {
        left_offset_override: arr3(v, "LeftOffsetOverride"),
        left_rotation_override: arr3(v, "LeftRotationOverride"),
        right_offset_override: arr3(v, "RightOffsetOverride"),
        right_rotation_override: arr3(v, "RightRotationOverride"),
    }
}

pub fn build(ctx: &Ctx) -> Result<UpdateItemPlayerAnimations> {
    let store = ctx.store(PATH)?;
    let list = store
        .own
        .keys()
        .map(|k| {
            let a = store.inherited(k, INHERITED);
            let animations = store
                .combined_map(k, "Animations")
                .map(|m| m.iter().map(|(n, v)| (n.clone(), animation(v))).collect())
                .unwrap_or_default();
            (
                k.clone(),
                ItemPlayerAnimations {
                    id: Some(k.clone()),
                    animations,
                    wiggle_weights: a.obj("WiggleWeights").map(wiggle),
                    camera: a.obj("Camera").map(camera),
                    pullback_config: a.obj("PullbackConfig").map(pullback),
                    use_first_person_override: a.bool_or("UseFirstPersonOverrides", false),
                },
            )
        })
        .collect();
    Ok(UpdateItemPlayerAnimations {
        r#type: super::INIT,
        item_player_animations: Some(list),
    })
}
