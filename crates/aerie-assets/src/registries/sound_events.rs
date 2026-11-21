//! `UpdateSoundEvents` (65) from `Server/Audio/SoundEvents` (spec 07).
//! Built-in `EMPTY` at index 0 (defaults below). Other registries refer to
//! sound events by index.
//!
//! Inherited from `Parent`: `Volume` (dB → gain, 0 dB), `Pitch` (semitones →
//! `1 / 2^(-st/12)`, 0), `StartAttenuationDistance` (2), `MaxDistance` (16),
//! `SpatialBlend` (0.6), `MaxInstance` (50), `PreventSoundInterruption`
//! (false), `Layers`, `AudioCategory` (→ index in AudioCategories, 0 when
//! absent). Not inherited: `StateBindings`, `DuckingRules`, `BypassDucking`.
//!
//! Layer: `Volume` (dB, 0), `StartDelay` (0), `FadeIn` (0), `Looping`
//! (false), `Probability` (100), `ProbabilityRerollDelay` (1),
//! `RoundRobinHistorySize` (0), `Files`, `RandomSettings` {`MinVolume`,
//! `MaxVolume` (dB, 0), `MinPitch`, `MaxPitch` (semitones, 0),
//! `MaxStartOffset` (0)}. An event without layers sends none.

use aerie_protocol::proto::{
    SoundEvent, SoundEventLayer, SoundEventLayerRandomSettings, UpdateSoundEvents,
};
use serde_json::Value;

use super::audio_effects::db_gain;
use super::audio_states;
use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub const PATH: &str = "Audio/SoundEvents";
pub const BUILTIN: &[&str] = &["EMPTY"];
const INHERITED: &[&str] = &[
    "Volume",
    "Pitch",
    "StartAttenuationDistance",
    "MaxDistance",
    "SpatialBlend",
    "MaxInstance",
    "PreventSoundInterruption",
    "Layers",
    "AudioCategory",
];

static NULL: Value = Value::Null;

/// Index of a sound event; 0 (`EMPTY`) when absent or unknown.
pub fn index(ctx: &Ctx, key: Option<&str>) -> Result<i32> {
    ctx.index_or(PATH, BUILTIN, key, 0)
}

/// Semitones to a pitch factor, as the official server computes it.
pub fn semitones(st: f32) -> f32 {
    (1.0 / 2f64.powf((-st / 12.0) as f64)) as f32
}

fn layer(v: &Value) -> SoundEventLayer {
    let r = v.get("RandomSettings").unwrap_or(&NULL);
    SoundEventLayer {
        volume: db_gain(v.f32_or("Volume", 0.0)),
        start_delay: v.f32_or("StartDelay", 0.0),
        fade_in: v.f32_or("FadeIn", 0.0),
        looping: v.bool_or("Looping", false),
        probability: v.i32_or("Probability", 100),
        probability_reroll_delay: v.f32_or("ProbabilityRerollDelay", 1.0),
        round_robin_history_size: v.i32_or("RoundRobinHistorySize", 0),
        random_settings: SoundEventLayerRandomSettings {
            min_volume: r.opt_f32("MinVolume").map_or(1.0, db_gain),
            max_volume: r.opt_f32("MaxVolume").map_or(1.0, db_gain),
            min_pitch: r.opt_f32("MinPitch").map_or(1.0, semitones),
            max_pitch: r.opt_f32("MaxPitch").map_or(1.0, semitones),
            max_start_offset: r.f32_or("MaxStartOffset", 0.0),
        },
        files: v.strings("Files").unwrap_or_default(),
    }
}

fn event(ctx: &Ctx, id: &str, a: &Value) -> Result<SoundEvent> {
    let layers: Vec<SoundEventLayer> = a
        .get("Layers")
        .and_then(Value::as_array)
        .map(|l| l.iter().map(layer).collect())
        .unwrap_or_default();
    let category = match a.str_at("AudioCategory") {
        Some(c) => ctx.index_or(
            audio_states::CATEGORIES,
            audio_states::CATEGORIES_BUILTIN,
            Some(&c),
            i32::MIN,
        )?,
        None => 0,
    };
    Ok(SoundEvent {
        id: Some(id.to_owned()),
        volume: a.opt_f32("Volume").map_or(1.0, db_gain),
        pitch: a.opt_f32("Pitch").map_or(1.0, semitones),
        max_instance: a.i32_or("MaxInstance", 50),
        prevent_sound_interruption: a.bool_or("PreventSoundInterruption", false),
        start_attenuation_distance: a.f32_or("StartAttenuationDistance", 2.0),
        max_distance: a.f32_or("MaxDistance", 16.0),
        spatial_blend: a.f32_or("SpatialBlend", 0.6),
        layers: (!layers.is_empty()).then_some(layers),
        audio_category: category,
        state_bindings: audio_states::bindings(ctx, a)?,
        ducking_rules: audio_states::ducking(ctx, a)?,
        bypass_ducking: a.bool_or("BypassDucking", false),
    })
}

pub fn build(ctx: &Ctx) -> Result<UpdateSoundEvents> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let mut map = Vec::new();
    for (i, k) in order.iter().enumerate() {
        let a = if store.own.contains_key(k) {
            store.inherited(k, INHERITED)
        } else {
            NULL.clone()
        };
        map.push((i as i32, event(ctx, k, &a)?));
    }
    Ok(UpdateSoundEvents {
        r#type: super::INIT,
        max_id: order.len() as i32,
        sound_events: Some(map),
    })
}
