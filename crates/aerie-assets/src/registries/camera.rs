//! Camera shake settings shared by `UpdateCameraShake` and `UpdateViewBobbing` (spec 07).
//!
//! Shake config: `Duration` (0), `StartTime` (absent → continuous, start 0),
//! `EaseIn`/`EaseOut` `{Time (0), Type (Linear)}`, `Offset {X,Y,Z}` and
//! `Rotation {Pitch,Yaw,Roll}`, each a list of noises (empty when absent).
//! Noise: `Seed` (0), `Type` (Sin), `Frequency` (0), `Amplitude` (0),
//! `Clamp {Min (-1), Max (1), Normalize (true)}`.

use aerie_protocol::proto::{
    CameraShakeConfig, ClampConfig, EasingConfig, EasingType, NoiseConfig, NoiseType, OffsetNoise,
    RotationNoise,
};
use serde_json::Value;

use crate::ctx::enum_or;
use crate::values::Get;

static NULL: Value = Value::Null;

fn easing(v: Option<&Value>) -> EasingConfig {
    let v = v.unwrap_or(&NULL);
    EasingConfig {
        time: v.f32_or("Time", 0.0),
        r#type: enum_or(v, "Type", EasingType::from_name, EasingType::LINEAR),
    }
}

fn noise(v: &Value) -> NoiseConfig {
    let c = v.get("Clamp").unwrap_or(&NULL);
    NoiseConfig {
        seed: v.i32_or("Seed", 0),
        r#type: enum_or(v, "Type", NoiseType::from_name, NoiseType::SIN),
        frequency: v.f32_or("Frequency", 0.0),
        amplitude: v.f32_or("Amplitude", 0.0),
        clamp: ClampConfig {
            min: c.f32_or("Min", -1.0),
            max: c.f32_or("Max", 1.0),
            normalize: c.bool_or("Normalize", true),
        },
    }
}

fn noises(v: Option<&Value>, key: &str) -> Vec<NoiseConfig> {
    v.and_then(|v| v.get(key))
        .and_then(Value::as_array)
        .map(|a| a.iter().map(noise).collect())
        .unwrap_or_default()
}

pub fn shake(v: Option<&Value>) -> CameraShakeConfig {
    let v = v.unwrap_or(&NULL);
    let start = v.opt_f32("StartTime");
    let (o, r) = (v.get("Offset"), v.get("Rotation"));
    CameraShakeConfig {
        duration: v.f32_or("Duration", 0.0),
        start_time: start.unwrap_or(0.0),
        continuous: start.is_none(),
        ease_in: easing(v.get("EaseIn")),
        ease_out: easing(v.get("EaseOut")),
        offset: OffsetNoise {
            x: noises(o, "X"),
            y: noises(o, "Y"),
            z: noises(o, "Z"),
        },
        rotation: RotationNoise {
            pitch: noises(r, "Pitch"),
            yaw: noises(r, "Yaw"),
            roll: noises(r, "Roll"),
        },
    }
}
