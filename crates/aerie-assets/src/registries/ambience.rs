//! `UpdateAmbienceFX` (62) from `Audio/AmbienceFX`, indexed, built-in `Empty`
//! first (spec 07). Inheritance follows the schema. Unknown keys give
//! `i32::MIN` unless noted.
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | key | `id` | |
//! | `Conditions` | `conditions` | absent |
//! | `Sounds` | `sounds` | absent when empty |
//! | `MusicContainer` (key or inline) | `music_container_index`; an effect with legacy `Music` uses `_legacy_<key>` | 0 |
//! | `AmbientBed` `{Track, Volume (dB), TransitionSpeed (Default), StateBindings}` | `ambient_bed` | absent |
//! | `SoundEffect` `{ReverbEffectId, EqualizerEffectId, IsInstant}` | `sound_effect` (indexes, 0 when absent) | absent |
//! | `Priority` | same | 0 |
//! | `BlockedAmbienceFxIds` | `blocked_ambience_fx_indices` | absent |
//! | `AudioCategory` | `audio_category_index` | 0 |
//! | `SetStates` `[{AudioState, Value, TransitionOverride}]` | `set_states` (-1 for unknown state or value) | absent when empty |
//!
//! Conditions: `Never`; `EnvironmentIds`, `WeatherIds` (absent when empty),
//! `FluidFXIds` → indexes; `EnvironmentTagPattern`, `WeatherTagPattern`,
//! `RoofMaterialTagPattern` (key or inline) → tag pattern index (-1 when
//! absent); `SurroundingBlockSoundSets` `[{BlockSoundSetId, Percent}]`;
//! `Altitude` (0-512), `Walls` (0-4), `SunLightLevel` / `TorchLightLevel` /
//! `GlobalLightLevel` (0-15), `DayTime` (0-24) ranges; `Roof`, `Floor`;
//! `Space`, `Shelter`, `Surfaces` (enum lists); `RoofState` (Any); the
//! `…Range` float ranges; `SurfacePhysicalMaterials` and
//! `ExteriorRoofPhysicalMaterials` `[{PhysicalMaterialId, Percent}]` with
//! their `…MatchAny` flags.
//!
//! Sound: `SoundEventId`, `BlockSoundSetId` → indexes (0 when absent),
//! `Play3D` (Random), `Altitude` (Normal), `Frequency` (1-10), `Radius`
//! (0-24), `MaxBodiesPerEmitter` (8), `SunlightRange`, `StateBindings`.

use aerie_protocol::proto::{
    AmbienceFX, AmbienceFXAltitude, AmbienceFXAmbientBed, AmbienceFXBlockSoundSet,
    AmbienceFXConditions, AmbienceFXPhysicalMaterial, AmbienceFXSound, AmbienceFXSoundEffect,
    AmbienceFXSoundPlay3D, AmbienceStateWrite, AmbienceTransitionSpeed, Range, Rangeb, Rangef,
    RoofState, ShelterType, SpaceSize, SurfaceType, UpdateAmbienceFX,
};
use serde_json::Value;

use super::audio_effects::{self, db_gain};
use super::particles::rangef;
use super::{
    audio_states, environments, materials, music, sound_events, sound_sets, tag_patterns, weathers,
};
use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::store::Store;
use crate::values::Get;

pub const PATH: &str = "Audio/AmbienceFX";
pub const BUILTIN: &[&str] = &["Empty"];

fn range(v: Option<&Value>, def: (i32, i32)) -> Range {
    let v = v.filter(|v| v.is_object());
    Range {
        min: v.map_or(def.0, |v| v.i32_or("Min", 0)),
        max: v.map_or(def.1, |v| v.i32_or("Max", 0)),
    }
}

fn rangeb(v: Option<&Value>, def: (i8, i8)) -> Rangeb {
    let v = v.filter(|v| v.is_object());
    Rangeb {
        min: v.map_or(def.0, |v| v.i32_or("Min", 0) as i8),
        max: v.map_or(def.1, |v| v.i32_or("Max", 0) as i8),
    }
}

fn indexes(
    ctx: &Ctx,
    v: &Value,
    key: &str,
    path: &str,
    builtin: &[&str],
) -> Result<Option<Vec<i32>>> {
    let Some(a) = v.get(key).and_then(Value::as_array) else {
        return Ok(None);
    };
    a.iter()
        .map(|x| ctx.index_or(path, builtin, x.as_str(), i32::MIN))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn enums<T>(v: &Value, key: &str, from: fn(&str) -> Option<T>) -> Option<Vec<T>> {
    v.get(key)?
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().and_then(from)).collect())
}

fn tag_pattern(ctx: &Ctx, c: &Value, owner: &str, field: &str) -> Result<i32> {
    match c
        .get(field)
        .and_then(|p| music::reference(p, owner, &format!("Conditions_{field}")))
    {
        Some(k) => ctx.index_or(tag_patterns::PATH, &[], Some(&k), i32::MIN),
        None => Ok(-1),
    }
}

fn materials_list(
    ctx: &Ctx,
    c: &Value,
    key: &str,
) -> Result<Option<Vec<AmbienceFXPhysicalMaterial>>> {
    let Some(l) = c
        .get(key)
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
    else {
        return Ok(None);
    };
    l.iter()
        .map(|m| {
            Ok(AmbienceFXPhysicalMaterial {
                physical_material_index: ctx.index_or(
                    materials::MATERIALS,
                    materials::MATERIALS_BUILTIN,
                    m.str_at("PhysicalMaterialId").as_deref(),
                    i32::MIN,
                )?,
                percent: m.obj("Percent").map(rangef),
            })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn conditions(ctx: &Ctx, c: &Value, owner: &str) -> Result<AmbienceFXConditions> {
    let non_empty = |v: Option<Vec<i32>>| v.filter(|v| !v.is_empty());
    let block_sets = match c.get("SurroundingBlockSoundSets").and_then(Value::as_array) {
        Some(l) if !l.is_empty() => Some(
            l.iter()
                .map(|s| {
                    Ok(AmbienceFXBlockSoundSet {
                        block_sound_set_index: ctx.index_or(
                            sound_sets::BLOCK,
                            sound_sets::EMPTY,
                            s.str_at("BlockSoundSetId").as_deref(),
                            i32::MIN,
                        )?,
                        percent: s.obj("Percent").map(rangef),
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        ),
        _ => None,
    };
    let light = (0, 15);
    Ok(AmbienceFXConditions {
        never: c.bool_or("Never", false),
        environment_indices: non_empty(indexes(
            ctx,
            c,
            "EnvironmentIds",
            environments::PATH,
            environments::BUILTIN,
        )?),
        weather_indices: non_empty(indexes(
            ctx,
            c,
            "WeatherIds",
            weathers::PATH,
            weathers::BUILTIN,
        )?),
        fluid_fx_indices: indexes(
            ctx,
            c,
            "FluidFXIds",
            materials::FLUID_FX,
            materials::FLUID_FX_BUILTIN,
        )?,
        environment_tag_pattern_index: tag_pattern(ctx, c, owner, "EnvironmentTagPattern")?,
        weather_tag_pattern_index: tag_pattern(ctx, c, owner, "WeatherTagPattern")?,
        surrounding_block_sound_sets: block_sets,
        altitude: range(c.get("Altitude"), (0, 512)),
        walls: rangeb(c.get("Walls"), (0, 4)),
        roof: c.bool_or("Roof", false),
        roof_material_tag_pattern_index: tag_pattern(ctx, c, owner, "RoofMaterialTagPattern")?,
        floor: c.bool_or("Floor", false),
        sun_light_level: rangeb(c.get("SunLightLevel"), light),
        torch_light_level: rangeb(c.get("TorchLightLevel"), light),
        global_light_level: rangeb(c.get("GlobalLightLevel"), light),
        day_time: c.obj("DayTime").map(rangef).unwrap_or(Rangef {
            min: 0.0,
            max: 24.0,
        }),
        space: enums(c, "Space", SpaceSize::from_name),
        shelter: enums(c, "Shelter", ShelterType::from_name),
        surfaces: enums(c, "Surfaces", SurfaceType::from_name),
        roof_state: enum_or(c, "RoofState", RoofState::from_name, RoofState::ANY),
        space_scale_range: c.obj("SpaceScaleRange").map(rangef),
        space_scale_min_range: c.obj("SpaceScaleMinRange").map(rangef),
        space_scale_max_range: c.obj("SpaceScaleMaxRange").map(rangef),
        escaped_ray_percent_range: c.obj("EscapedRayPercentRange").map(rangef),
        reflection_coeff_range: c.obj("ReflectionCoeffRange").map(rangef),
        absorption_coeff_range: c.obj("AbsorptionCoeffRange").map(rangef),
        roof_distance_range: c.obj("RoofDistanceRange").map(rangef),
        surface_physical_materials: materials_list(ctx, c, "SurfacePhysicalMaterials")?,
        surface_physical_materials_match_any: c.bool_or("SurfacePhysicalMaterialsMatchAny", false),
        exterior_roof_physical_materials: materials_list(ctx, c, "ExteriorRoofPhysicalMaterials")?,
        exterior_roof_physical_materials_match_any: c
            .bool_or("ExteriorRoofPhysicalMaterialsMatchAny", false),
    })
}

fn sound(ctx: &Ctx, s: &Value) -> Result<AmbienceFXSound> {
    Ok(AmbienceFXSound {
        sound_event_index: match s.str_at("SoundEventId") {
            Some(id) => ctx.index_or(
                sound_events::PATH,
                sound_events::BUILTIN,
                Some(&id),
                i32::MIN,
            )?,
            None => 0,
        },
        play3_d: enum_or(
            s,
            "Play3D",
            AmbienceFXSoundPlay3D::from_name,
            AmbienceFXSoundPlay3D::RANDOM,
        ),
        block_sound_set_index: match s.str_at("BlockSoundSetId") {
            Some(id) => ctx.index_or(sound_sets::BLOCK, sound_sets::EMPTY, Some(&id), i32::MIN)?,
            None => 0,
        },
        altitude: enum_or(
            s,
            "Altitude",
            AmbienceFXAltitude::from_name,
            AmbienceFXAltitude::NORMAL,
        ),
        frequency: s.obj("Frequency").map(rangef).unwrap_or(Rangef {
            min: 1.0,
            max: 10.0,
        }),
        radius: range(s.get("Radius"), (0, 24)),
        max_bodies_per_emitter: s.i32_or("MaxBodiesPerEmitter", 8),
        sunlight_range: s.obj("SunlightRange").map(|r| rangeb(Some(r), (0, 0))),
        state_bindings: audio_states::bindings(ctx, s)?,
    })
}

fn set_states(ctx: &Ctx, a: &Value) -> Result<Option<Vec<AmbienceStateWrite>>> {
    let Some(l) = a
        .get("SetStates")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
    else {
        return Ok(None);
    };
    let states = ctx.store(audio_states::STATES)?;
    let mut out = Vec::new();
    for w in l.iter().filter(|w| w.is_object()) {
        let id = w.str_at("AudioState").filter(|s| !s.trim().is_empty());
        let index = match &id {
            Some(id) => ctx.index_or(
                audio_states::STATES,
                audio_states::STATES_BUILTIN,
                Some(id),
                i32::MIN,
            )?,
            None => i32::MIN,
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
        let (audio_state_index, value_index) = if index == i32::MIN {
            (-1, -1)
        } else {
            let v = audio_states::value_index(w.str_at("Value").as_deref(), &values);
            (index, if v >= 0 { v } else { -1 })
        };
        out.push(AmbienceStateWrite {
            audio_state_index,
            value_index,
            transition_override: w
                .obj("TransitionOverride")
                .map(|t| audio_states::transition(t, &values)),
        });
    }
    Ok(Some(out))
}

fn effect(ctx: &Ctx, store: &Store, k: &str) -> Result<AmbienceFX> {
    if !store.own.contains_key(k) {
        return Ok(AmbienceFX {
            id: Some(k.to_owned()),
            ..Default::default()
        });
    }
    let a = store.resolved(k, "AmbienceFX");
    let music_key = match a.get("MusicContainer") {
        Some(m) => music::reference(
            m,
            &music::owner_of(store, k, "MusicContainer"),
            "MusicContainer",
        ),
        None if a.get("Music").is_some_and(Value::is_object) => Some(format!("_legacy_{k}")),
        None => None,
    };
    let sounds = match a
        .get("Sounds")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
    {
        Some(l) => Some(
            l.iter()
                .map(|s| sound(ctx, s))
                .collect::<Result<Vec<_>>>()?,
        ),
        None => None,
    };
    Ok(AmbienceFX {
        id: Some(k.to_owned()),
        conditions: match a.obj("Conditions") {
            Some(c) => Some(conditions(
                ctx,
                c,
                &music::owner_of(store, k, "Conditions"),
            )?),
            None => None,
        },
        sounds,
        music_container_index: match &music_key {
            Some(m) => ctx.index_or(
                music::CONTAINERS,
                music::CONTAINERS_BUILTIN,
                Some(m),
                i32::MIN,
            )?,
            None => 0,
        },
        ambient_bed: match a.obj("AmbientBed") {
            Some(b) => Some(AmbienceFXAmbientBed {
                track: b.str_at("Track"),
                volume: db_gain(b.f32_or("Volume", 0.0)),
                transition_speed: enum_or(
                    b,
                    "TransitionSpeed",
                    AmbienceTransitionSpeed::from_name,
                    AmbienceTransitionSpeed::from_name("Default").unwrap_or_default(),
                ),
                state_bindings: audio_states::bindings(ctx, b)?,
            }),
            None => None,
        },
        sound_effect: match a.obj("SoundEffect") {
            Some(e) => Some(AmbienceFXSoundEffect {
                reverb_effect_index: match e.str_at("ReverbEffectId") {
                    Some(id) => ctx.index_or(
                        audio_effects::REVERB,
                        audio_effects::EMPTY,
                        Some(&id),
                        i32::MIN,
                    )?,
                    None => 0,
                },
                equalizer_effect_index: match e.str_at("EqualizerEffectId") {
                    Some(id) => {
                        ctx.index_or(audio_effects::EQ, audio_effects::EMPTY, Some(&id), i32::MIN)?
                    }
                    None => 0,
                },
                is_instant: e.bool_or("IsInstant", false),
            }),
            None => None,
        },
        priority: a.i32_or("Priority", 0),
        blocked_ambience_fx_indices: indexes(ctx, &a, "BlockedAmbienceFxIds", PATH, BUILTIN)?,
        audio_category_index: match a.str_at("AudioCategory") {
            Some(c) => ctx.index_or(
                audio_states::CATEGORIES,
                audio_states::CATEGORIES_BUILTIN,
                Some(&c),
                i32::MIN,
            )?,
            None => 0,
        },
        set_states: set_states(ctx, &a)?,
    })
}

pub fn build(ctx: &Ctx) -> Result<UpdateAmbienceFX> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    let mut list = Vec::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        list.push((i, effect(ctx, &store, k)?));
    }
    Ok(UpdateAmbienceFX {
        r#type: super::INIT,
        max_id: order.len() as i32,
        ambience_fx: Some(list),
    })
}
