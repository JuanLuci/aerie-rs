//! `UpdateAudioStates` (89) from `Server/Audio/AudioStates` and
//! `UpdateAudioCategories` (80) from `Server/Audio/AudioCategories` (spec 07).
//!
//! Audio state (built-in `EMPTY` at 0): `Authority` (enum name), `Values`
//! (names), `DefaultValue` (→ index in `Values`, 0 when absent or unknown),
//! `DefaultSyncTo`, `DefaultTransition`, `Transitions`, `RevertWhenInactive`.
//! Transition: `From`/`To` (value names → index; absent or `*` → -1, unknown
//! → -1), `Duration`/`Delay` (bar-beat durations `{Bars, Beats, Ms}`),
//! `Curve` (Linear), `SyncTo` (music sync `{Point, Marker, Offset,
//! BarMultiple}`), `PostEvent`.
//!
//! Audio category (built-in `EMPTY`, `AudioCat_Music`, `AudioCat_Ambient`,
//! `AudioCat_SFX`, `AudioCat_Voice`, `AudioCat_UI` at 0-5): `Volume` (dB → gain,
//! 0 dB), parent (index of the `Parent` category, -1 without one),
//! `StateBindings` (`AudioState` → index, `Deltas`: `Value` → index in that
//! state's values, `VolumeDb`, `Mute`, `PitchSemitones`, `Fade`),
//! `DuckingRules` (`TargetCategory` → index, `DuckingVolumeDb`, `AttackMs`,
//! `HoldMs`, `ReleaseMs`, `Curve`, `ReleaseCurve` (Linear), `Priority`),
//! `MaxDuckingDb` (-100). Empty lists are left out.

use aerie_protocol::proto::{
    AudioCategory, AudioCategoryDuckingRule, AudioState, AudioStateAuthority, BarBeatDuration,
    FadeCurve, MusicSync, StateBinding, StateDelta, StateFade, StateTransition, SyncPoint,
    UpdateAudioCategories, UpdateAudioStates,
};
use serde_json::Value;

use super::audio_effects::db_gain;
use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const STATES: &str = "Audio/AudioStates";
pub const STATES_BUILTIN: &[&str] = &["EMPTY"];
pub const CATEGORIES: &str = "Audio/AudioCategories";
pub const CATEGORIES_BUILTIN: &[&str] = &[
    "EMPTY",
    "AudioCat_Music",
    "AudioCat_Ambient",
    "AudioCat_SFX",
    "AudioCat_Voice",
    "AudioCat_UI",
];

static NULL: Value = Value::Null;

fn list(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn non_empty<T>(v: Vec<T>) -> Option<Vec<T>> {
    (!v.is_empty()).then_some(v)
}

pub fn bar_beat(v: &Value) -> BarBeatDuration {
    BarBeatDuration {
        bars: v.i32_or("Bars", 0),
        beats: v.f32_or("Beats", 0.0),
        ms: v.f32_or("Ms", 0.0),
    }
}

pub fn music_sync(v: &Value) -> MusicSync {
    MusicSync {
        point: enum_or(v, "Point", SyncPoint::from_name, SyncPoint::IMMEDIATE),
        marker_name: v.str_at("Marker"),
        offset: v.obj("Offset").map(bar_beat),
        bar_multiple: v.i32_or("BarMultiple", 0),
    }
}

fn curve(v: &Value, key: &str) -> FadeCurve {
    enum_or(v, key, FadeCurve::from_name, FadeCurve::LINEAR)
}

/// Index of `name` in `values`: -1 for absent or `*`, -2 when not found.
pub fn value_index(name: Option<&str>, values: &[String]) -> i32 {
    match name {
        None | Some("*") => -1,
        Some(n) => values.iter().position(|v| v == n).map_or(-2, |i| i as i32),
    }
}

pub fn transition(t: &Value, values: &[String]) -> StateTransition {
    let idx = |k: &str| value_index(t.str_at(k).as_deref(), values).max(-1);
    StateTransition {
        from_value_index: idx("From"),
        to_value_index: idx("To"),
        duration: t.obj("Duration").map(bar_beat),
        curve: curve(t, "Curve"),
        sync_to: t.obj("SyncTo").map(music_sync),
        delay: t.obj("Delay").map(bar_beat),
        post_event: t.str_at("PostEvent"),
    }
}

fn state(id: &str, a: &Value) -> AudioState {
    let values = a.strings("Values");
    let vals = values.clone().unwrap_or_default();
    AudioState {
        id: Some(id.to_owned()),
        authority: enum_or(
            a,
            "Authority",
            AudioStateAuthority::from_name,
            AudioStateAuthority::SERVER,
        ),
        default_value_index: value_index(a.str_at("DefaultValue").as_deref(), &vals).max(0),
        values,
        default_sync_to: a.obj("DefaultSyncTo").map(music_sync),
        default_transition: a.obj("DefaultTransition").map(|t| transition(t, &vals)),
        transitions: a
            .get("Transitions")
            .and_then(Value::as_array)
            .map(|ts| ts.iter().map(|t| transition(t, &vals)).collect()),
        revert_when_inactive: a.bool_or("RevertWhenInactive", false),
    }
}

pub fn build_states(ctx: &Ctx) -> Result<UpdateAudioStates> {
    let store = ctx.store(STATES)?;
    let order = ctx.order(STATES, STATES_BUILTIN)?;
    let map = order
        .iter()
        .enumerate()
        .map(|(i, k)| (i as i32, state(k, store.assets.get(k).unwrap_or(&NULL))))
        .collect();
    Ok(UpdateAudioStates {
        r#type: super::INIT,
        max_id: order.len() as i32,
        audio_states: Some(map),
    })
}

/// State bindings, shared with other audio assets.
pub fn bindings(ctx: &Ctx, v: &Value) -> Result<Option<Vec<StateBinding>>> {
    let states = ctx.store(STATES)?;
    let mut out = Vec::new();
    for b in list(v, "StateBindings") {
        let id = b.str_at("AudioState").filter(|s| !s.trim().is_empty());
        let index = match &id {
            Some(id) => ctx.index_or(STATES, STATES_BUILTIN, Some(id), -1)?,
            None => -1,
        };
        let values: Vec<String> = id
            .as_ref()
            .and_then(|id| {
                states
                    .assets
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(id))
            })
            .and_then(|(_, s)| s.strings("Values"))
            .unwrap_or_default();
        let deltas = b.get("Deltas").and_then(Value::as_array).map(|ds| {
            ds.iter()
                .map(|d| StateDelta {
                    value_index: if index < 0 {
                        -1
                    } else {
                        value_index(d.str_at("Value").as_deref(), &values).max(-1)
                    },
                    volume_db: d.f32_or("VolumeDb", 0.0),
                    mute: d.bool_or("Mute", false),
                    pitch_semitones: d.f32_or("PitchSemitones", 0.0),
                    fade: d.obj("Fade").map(|f| StateFade {
                        duration: f.obj("Duration").map(bar_beat),
                        curve: curve(f, "Curve"),
                        delay: f.obj("Delay").map(bar_beat),
                    }),
                })
                .collect()
        });
        out.push(StateBinding {
            audio_state_index: index,
            deltas,
        });
    }
    Ok(non_empty(out))
}

/// Ducking rules, shared with other audio assets.
pub fn ducking(ctx: &Ctx, v: &Value) -> Result<Option<Vec<AudioCategoryDuckingRule>>> {
    let mut out = Vec::new();
    for r in list(v, "DuckingRules") {
        out.push(AudioCategoryDuckingRule {
            target_audio_category_index: ctx.index_or(
                CATEGORIES,
                CATEGORIES_BUILTIN,
                r.str_at("TargetCategory").as_deref(),
                i32::MIN,
            )?,
            ducking_volume_db: r.f32_or("DuckingVolumeDb", 0.0),
            attack_ms: r.f32_or("AttackMs", 0.0),
            hold_ms: r.f32_or("HoldMs", 0.0),
            release_ms: r.f32_or("ReleaseMs", 0.0),
            curve: curve(&r, "Curve"),
            release_curve: curve(&r, "ReleaseCurve"),
            priority: r.i32_or("Priority", 0) as i16,
        });
    }
    Ok(non_empty(out))
}

pub fn build_categories(ctx: &Ctx) -> Result<UpdateAudioCategories> {
    let store = ctx.store(CATEGORIES)?;
    let order = ctx.order(CATEGORIES, CATEGORIES_BUILTIN)?;
    let mut map = Vec::new();
    for (i, k) in order.iter().enumerate() {
        // No audio category field inherits; `Parent` only sets the parent index.
        let a = store.own.get(k).unwrap_or(&NULL);
        let parent = match store.parents.get(k) {
            Some(p) => ctx.index_or(CATEGORIES, CATEGORIES_BUILTIN, Some(p), -1)?,
            None => -1,
        };
        map.push((
            i as i32,
            AudioCategory {
                id: Some(k.clone()),
                volume: db_gain(a.opt_f32("Volume").unwrap_or(0.0)),
                parent_audio_category_index: parent,
                state_bindings: bindings(ctx, a)?,
                ducking_rules: ducking(ctx, a)?,
                max_ducking_db: a.f32_or("MaxDuckingDb", -100.0),
            },
        ));
    }
    Ok(UpdateAudioCategories {
        r#type: super::INIT,
        max_id: order.len() as i32,
        categories: Some(map),
    })
}
