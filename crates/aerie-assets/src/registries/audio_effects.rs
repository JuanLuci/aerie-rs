//! `UpdateReverbEffects` (81) from `Server/Audio/Reverb` and
//! `UpdateEqualizerEffects` (82) from `Server/Audio/EQ` (spec 07).
//! Both have a built-in `EMPTY` at index 0 with the defaults below.
//!
//! Keys marked (dB) are written in decibels and sent as linear gain:
//! `gain = 10^(dB / 20)`, or 0 at -100 dB and below.
//!
//! Reverb: `DryGain` (dB, 0), `ModalDensity` (1), `Diffusion` (1),
//! `Gain` (dB, -10), `HighFrequencyGain` (dB, -1), `DecayTime` (1.49),
//! `HighFrequencyDecayRatio` (0.83), `ReflectionGain` (dB, -26),
//! `ReflectionDelay` (0.007), `LateReverbGain` (dB, 2), `LateReverbDelay`
//! (0.011), `RoomRolloffFactor` (0), `AirAbsorbptionHighFrequencyGain`
//! (dB, -0.05; the key is spelled that way), `LimitDecayHighFrequency` (true).
//!
//! Equalizer: `LowGain` (dB, 0), `LowCutOff` (200), `LowMidGain` (dB, 0),
//! `LowMidCenter` (500), `LowMidWidth` (1), `HighMidGain` (dB, 0),
//! `HighMidCenter` (3000), `HighMidWidth` (1), `HighGain` (dB, 0),
//! `HighCutOff` (6000).

use aerie_protocol::proto::{
    EqualizerEffect, ReverbEffect, UpdateEqualizerEffects, UpdateReverbEffects,
};
use serde_json::Value;

use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub const REVERB: &str = "Audio/Reverb";
pub const EQ: &str = "Audio/EQ";
pub const EMPTY: &[&str] = &["EMPTY"];

static NULL: Value = Value::Null;

/// Decibels to linear gain, computed the way the official server does (in f32, power in f64).
pub fn db_gain(db: f32) -> f32 {
    if db <= -100.0 {
        0.0
    } else {
        10f64.powf((db / 20.0) as f64) as f32
    }
}

fn db(v: &Value, key: &str, default_db: f32) -> f32 {
    db_gain(v.opt_f32(key).unwrap_or(default_db))
}

fn reverb(id: &str, v: &Value) -> ReverbEffect {
    ReverbEffect {
        id: Some(id.to_owned()),
        dry_gain: db(v, "DryGain", 0.0),
        modal_density: v.f32_or("ModalDensity", 1.0),
        diffusion: v.f32_or("Diffusion", 1.0),
        gain: db(v, "Gain", -10.0),
        high_frequency_gain: db(v, "HighFrequencyGain", -1.0),
        decay_time: v.f32_or("DecayTime", 1.49),
        high_frequency_decay_ratio: v.f32_or("HighFrequencyDecayRatio", 0.83),
        reflection_gain: db(v, "ReflectionGain", -26.0),
        reflection_delay: v.f32_or("ReflectionDelay", 0.007),
        late_reverb_gain: db(v, "LateReverbGain", 2.0),
        late_reverb_delay: v.f32_or("LateReverbDelay", 0.011),
        room_rolloff_factor: v.f32_or("RoomRolloffFactor", 0.0),
        air_absorption_high_frequency_gain: db(v, "AirAbsorbptionHighFrequencyGain", -0.05),
        limit_decay_high_frequency: v.bool_or("LimitDecayHighFrequency", true),
    }
}

fn eq(id: &str, v: &Value) -> EqualizerEffect {
    EqualizerEffect {
        id: Some(id.to_owned()),
        low_gain: db(v, "LowGain", 0.0),
        low_cut_off: v.f32_or("LowCutOff", 200.0),
        low_mid_gain: db(v, "LowMidGain", 0.0),
        low_mid_center: v.f32_or("LowMidCenter", 500.0),
        low_mid_width: v.f32_or("LowMidWidth", 1.0),
        high_mid_gain: db(v, "HighMidGain", 0.0),
        high_mid_center: v.f32_or("HighMidCenter", 3000.0),
        high_mid_width: v.f32_or("HighMidWidth", 1.0),
        high_gain: db(v, "HighGain", 0.0),
        high_cut_off: v.f32_or("HighCutOff", 6000.0),
    }
}

pub fn build_reverb(ctx: &Ctx) -> Result<UpdateReverbEffects> {
    let store = ctx.store(REVERB)?;
    let order = ctx.order(REVERB, EMPTY)?;
    let effects = order
        .iter()
        .enumerate()
        .map(|(i, k)| (i as i32, reverb(k, store.assets.get(k).unwrap_or(&NULL))))
        .collect();
    Ok(UpdateReverbEffects {
        r#type: super::INIT,
        max_id: order.len() as i32,
        effects: Some(effects),
    })
}

pub fn build_eq(ctx: &Ctx) -> Result<UpdateEqualizerEffects> {
    let store = ctx.store(EQ)?;
    let order = ctx.order(EQ, EMPTY)?;
    let effects = order
        .iter()
        .enumerate()
        .map(|(i, k)| (i as i32, eq(k, store.assets.get(k).unwrap_or(&NULL))))
        .collect();
    Ok(UpdateEqualizerEffects {
        r#type: super::INIT,
        max_id: order.len() as i32,
        effects: Some(effects),
    })
}
