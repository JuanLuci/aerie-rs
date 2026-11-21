//! Music registries (spec 07), all indexed with a built-in entry first;
//! inheritance follows the schema.
//!
//! **`UpdateMusicContainers` (88)** from `Audio/MusicContainers` (built-in
//! `Empty`, a single track), polymorphic by `Type` (`SingleTrack`, `Random`,
//! `Sequence`, `Horizontal`, `Segment`; a container naming only a `Parent`
//! takes its parent's type). Most containers are contained: children written
//! inline (`*Owner_Children_0`, `*Owner_Layers_0_Container`, a stinger's
//! `*Stinger_MusicContainer`). Every type has:
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | `Volume` (dB) | `volume` (gain) | 1 |
//! | `LoopCount`, `Weight` | same | 1 |
//! | `SilenceAfter`, `ExitSilence` `{Min, Max}` | same | absent |
//! | `FadeInDuration`, `FadeOutDuration`, `TransitionDuration` | same | 0 |
//! | `TransitionType` | `transition_type` | `FadeOutFadeIn` |
//! | `ExitAt` (music sync) | `exit_at` | absent |
//! | `Align`, `AlignMarker` | `align`, `align_marker_name` | absent |
//! | `PlayToCompletion` | same | false |
//! | `ResumeMemoryDuration` | same | 30 (random containers: 600) |
//! | `NameTranslationKey` | same | absent |
//! | `AudioCategory` | `audio_category_index` | 0 (also when unknown) |
//! | `Tempo` `{Bpm, BeatsPerBar, BeatValue}` | `tempo` | absent |
//! | `StateBindings` | `state_bindings` | absent |
//! | `Stingers` `[{MusicEvent, Stinger, PlayAt, MinRepeatIntervalSeconds, ShortLead}]` | `stingers` (stinger index; 0 when unknown) | absent |
//!
//! Then by type: `Track`; `Mode` (Shuffle), `AvoidRepeatCount` (0) and
//! `Children`; `Children`; `DefaultPhaseTransitionType` (Crossfade),
//! `DefaultPhaseTransitionDuration`, `Children`, `AudioState` → index (0),
//! `Phases` `[{Value, Phase}]` → `phase_value_indices` (one per value of the
//! audio state, -1 where no phase); `Layers` `[{Name, Container, ClipStart,
//! StateBindings}]`, `EntryMarker`, `ExitMarker`, `Markers` `[{Name, At {Bar,
//! Beat, Ms}}]` (sorted by time at the container's tempo). Child indexes are
//! 0 when unknown.
//!
//! **`UpdateStingers` (92)** from `Audio/Stingers` (built-in `EMPTY`):
//! `MusicContainer` → index, `MaxInstance` (1), `Priority`, `ExclusionGroup`,
//! `AudioCategory` → index, `Volume` (dB → gain, 1), `DuckingRules`.
//!
//! **`UpdateMusicTransitions` (91)** from `Audio/MusicTransitions` (built-in
//! `EMPTY`): `From` / `To` match `{Container}` (that container) or
//! `{TagPattern}` (every container whose tags satisfy it) → sorted container
//! indexes, absent without a match rule; `ExitAt`, `Align` (EntryMarker),
//! `AlignMarker`, `FadeOutDuration`, `FadeOutDelay`, `FadeInDuration`
//! (bar-beat durations), `FadeOutCurve` / `FadeInCurve` (EqualPowerSine),
//! `PlayPostExit`, `PostEvent`, `Priority`; `specificity` = 3 × from + to,
//! where a container match counts 2, a tag pattern 1, none 0.

use std::collections::HashSet;

use aerie_protocol::proto::{
    DestinationAlign, FadeCurve, HorizontalMusicContainer, LayerPlacement, MusicContainer,
    MusicMarker, MusicTransition, MusicTransitionType, RandomMode, RandomMusicContainer,
    SegmentMusicContainer, SequenceMusicContainer, SingleTrackMusicContainer, Stinger,
    StingerBinding, StingerShortLead, TempoSettings, UpdateMusicContainers, UpdateMusicTransitions,
    UpdateStingers,
};
use serde_json::Value;

use super::audio_effects::db_gain;
use super::audio_states::{self, bar_beat, music_sync};
use super::particles::rangef;
use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::store::Store;
use crate::values::Get;

pub const CONTAINERS: &str = "Audio/MusicContainers";
pub const CONTAINERS_BUILTIN: &[&str] = &["Empty"];
pub const STINGERS: &str = "Audio/Stingers";
pub const STINGERS_BUILTIN: &[&str] = &["EMPTY"];
pub const TRANSITIONS: &str = "Audio/MusicTransitions";
pub const TRANSITIONS_BUILTIN: &[&str] = &["EMPTY"];

static NULL: Value = Value::Null;

/// The asset in `store` that sets `field` itself: `key` or its nearest
/// ancestor. Inline assets in that field are keyed after it.
pub fn owner_of(store: &Store, key: &str, field: &str) -> String {
    let mut k = key.to_owned();
    for _ in 0..32 {
        if store.own.get(&k).is_some_and(|a| a.get(field).is_some()) {
            return k;
        }
        match store.parents.get(&k) {
            Some(p) => k = p.clone(),
            None => break,
        }
    }
    key.to_owned()
}

/// Key named by a field that may hold a contained asset: the string itself,
/// or the generated key of the inline asset (`path` below the owner).
pub fn reference(v: &Value, owner: &str, path: &str) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => Some(format!("*{owner}_{path}")),
        _ => None,
    }
}

fn container_index(ctx: &Ctx, key: Option<&str>) -> Result<i32> {
    ctx.index_or(CONTAINERS, CONTAINERS_BUILTIN, key, 0)
}

/// A container's type: its own `Type`, else its nearest ancestor's.
fn container_type(store: &Store, key: &str) -> String {
    let mut k = key.to_owned();
    for _ in 0..32 {
        if let Some(t) = store.own.get(&k).and_then(|a| a.str_at("Type")) {
            return t;
        }
        match store.parents.get(&k) {
            Some(p) => k = p.clone(),
            None => break,
        }
    }
    "SingleTrack".into()
}

/// Asset tag names (spec 07, "Tags"): each key, each value and `key=value`,
/// down the parent chain.
pub fn tag_names(store: &Store, key: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut k = Some(key.to_owned());
    let mut n = 0;
    while let Some(cur) = k {
        if let Some(Value::Object(tags)) = store.own.get(&cur).and_then(|a| a.get("Tags")) {
            for (t, vals) in tags {
                out.insert(t.clone());
                for v in vals
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    out.insert(v.to_owned());
                    out.insert(format!("{t}={v}"));
                }
            }
        }
        n += 1;
        k = store.parents.get(&cur).cloned().filter(|_| n < 32);
    }
    out
}

/// Whether a tag pattern (JSON) holds for a set of tag names.
pub fn pattern_matches(p: &Value, tags: &HashSet<String>) -> bool {
    match p.str_at("Op").as_deref() {
        Some("And") => p
            .get("Patterns")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().all(|x| pattern_matches(x, tags))),
        Some("Or") => p
            .get("Patterns")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|x| pattern_matches(x, tags))),
        Some("Not") => p.get("Pattern").is_some_and(|x| !pattern_matches(x, tags)),
        _ => p.str_at("Tag").is_some_and(|t| tags.contains(&t)),
    }
}

fn stinger_bindings(ctx: &Ctx, a: &Value) -> Result<Option<Vec<StingerBinding>>> {
    let Some(list) = a.get("Stingers").and_then(Value::as_array) else {
        return Ok(None);
    };
    let stingers = ctx.store(STINGERS)?;
    let mut out = Vec::new();
    for b in list.iter().filter(|b| b.is_object()) {
        let id = b.str_at("Stinger");
        let known = id.as_ref().is_some_and(|s| {
            stingers.own.keys().any(|k| k.eq_ignore_ascii_case(s))
                || s.eq_ignore_ascii_case("EMPTY")
        });
        out.push(StingerBinding {
            music_event: b.str_at("MusicEvent"),
            stinger_index: if known {
                ctx.index_or(STINGERS, STINGERS_BUILTIN, id.as_deref(), 0)?
            } else {
                0
            },
            play_at: b.obj("PlayAt").map(music_sync),
            min_repeat_interval_seconds: b.f32_or("MinRepeatIntervalSeconds", 0.0),
            short_lead: enum_or(
                b,
                "ShortLead",
                StingerShortLead::from_name,
                StingerShortLead::DROP_PICKUP,
            ),
        });
    }
    Ok((!out.is_empty()).then_some(out))
}

fn children(ctx: &Ctx, store: &Store, key: &str, a: &Value) -> Result<Option<Vec<i32>>> {
    let Some(list) = a
        .get("Children")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
    else {
        return Ok(None);
    };
    let owner = owner_of(store, key, "Children");
    list.iter()
        .enumerate()
        .map(|(i, c)| {
            container_index(
                ctx,
                reference(c, &owner, &format!("Children_{i}")).as_deref(),
            )
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// The fields every container type shares.
macro_rules! base {
    ($ty:ident, $ctx:expr, $a:expr, { $($extra:tt)* }) => {
        base!($ty, $ctx, $a, 30.0, { $($extra)* })
    };
    ($ty:ident, $ctx:expr, $a:expr, $resume:expr, { $($extra:tt)* }) => {
        $ty {
            volume: $a.get("Volume").and_then(Value::as_f64).map(|d| db_gain(d as f32)).unwrap_or(1.0),
            loop_count: $a.i32_or("LoopCount", 1),
            weight: $a.f32_or("Weight", 1.0),
            silence_after: $a.obj("SilenceAfter").map(rangef),
            exit_silence: $a.obj("ExitSilence").map(rangef),
            fade_in_duration: $a.f32_or("FadeInDuration", 0.0),
            fade_out_duration: $a.f32_or("FadeOutDuration", 0.0),
            transition_type: enum_or($a, "TransitionType", MusicTransitionType::from_name, MusicTransitionType::FADE_OUT_FADE_IN),
            transition_duration: $a.f32_or("TransitionDuration", 0.0),
            exit_at: $a.obj("ExitAt").map(music_sync),
            align: $a.str_at("Align").as_deref().and_then(DestinationAlign::from_name),
            align_marker_name: $a.str_at("AlignMarker"),
            play_to_completion: $a.bool_or("PlayToCompletion", false),
            resume_memory_duration: $a.f32_or("ResumeMemoryDuration", $resume),
            name_translation_key: $a.str_at("NameTranslationKey"),
            audio_category_index: $ctx.index_or(
                audio_states::CATEGORIES,
                audio_states::CATEGORIES_BUILTIN,
                $a.str_at("AudioCategory").as_deref(),
                0,
            )?,
            tempo: $a.obj("Tempo").map(|t| TempoSettings {
                bpm: t.f32_or("Bpm", 0.0),
                beats_per_bar: t.i32_or("BeatsPerBar", 0),
                beat_value: t.i32_or("BeatValue", 0),
            }),
            state_bindings: audio_states::bindings($ctx, $a)?,
            stingers: stinger_bindings($ctx, $a)?,
            $($extra)*
        }
    };
}

fn marker_ms(at: &Value, tempo: Option<&Value>) -> f32 {
    let bar = at.i32_or("Bar", 1);
    let beat = at.i32_or("Beat", 1);
    let ms = at.f32_or("Ms", 0.0);
    let Some(t) = tempo else { return ms };
    let bpm = t.f32_or("Bpm", 0.0);
    if bpm <= 0.0 {
        return ms;
    }
    let beat_value = t.i32_or("BeatValue", 0);
    let per_beat = if beat_value <= 0 {
        60000.0 / bpm
    } else {
        60000.0 / bpm * (4.0 / beat_value as f32)
    };
    ((bar - 1) * t.i32_or("BeatsPerBar", 0) + (beat - 1)) as f32 * per_beat + ms
}

fn container(ctx: &Ctx, store: &Store, key: &str) -> Result<MusicContainer> {
    let a = &store.resolved(key, "MusicContainer");
    Ok(match container_type(store, key).as_str() {
        "Random" => MusicContainer::RandomMusicContainer(Box::new(
            base!(RandomMusicContainer, ctx, a, 600.0, {
                mode: enum_or(a, "Mode", RandomMode::from_name, RandomMode::SHUFFLE),
                avoid_repeat_count: a.i32_or("AvoidRepeatCount", 0),
                children: children(ctx, store, key, a)?,
            }),
        )),
        "Sequence" => MusicContainer::SequenceMusicContainer(Box::new(
            base!(SequenceMusicContainer, ctx, a, {
                children: children(ctx, store, key, a)?,
            }),
        )),
        "Horizontal" => {
            let state = a.str_at("AudioState");
            let values = match &state {
                Some(s) => ctx
                    .store(audio_states::STATES)?
                    .assets
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(s))
                    .and_then(|(_, v)| v.strings("Values")),
                None => None,
            };
            let phases = a
                .get("Phases")
                .and_then(Value::as_array)
                .filter(|p| !p.is_empty());
            let phase_value_indices = match (&state, phases, values.filter(|v| !v.is_empty())) {
                (Some(_), Some(phases), Some(values)) => {
                    let mut out = vec![-1; values.len()];
                    for p in phases.iter().filter(|p| p.is_object()) {
                        let v = p.str_at("Value");
                        if let Some(i) = v
                            .as_deref()
                            .filter(|v| *v != "*")
                            .and_then(|v| values.iter().position(|x| x == v))
                        {
                            out[i] = p.i32_or("Phase", 0);
                        }
                    }
                    Some(out)
                }
                _ => None,
            };
            MusicContainer::HorizontalMusicContainer(Box::new(
                base!(HorizontalMusicContainer, ctx, a, {
                    default_phase_transition_type: enum_or(a, "DefaultPhaseTransitionType", MusicTransitionType::from_name, MusicTransitionType::CROSSFADE),
                    default_phase_transition_duration: a.f32_or("DefaultPhaseTransitionDuration", 0.0),
                    children: children(ctx, store, key, a)?,
                    audio_state_index: match &state {
                        Some(s) => ctx.index_or(audio_states::STATES, audio_states::STATES_BUILTIN, Some(s), 0)?,
                        None => 0,
                    },
                    phase_value_indices,
                }),
            ))
        }
        "Segment" => {
            let tempo = a.obj("Tempo");
            let markers = a.get("Markers").and_then(Value::as_array).and_then(|m| {
                let mut list: Vec<&Value> = m.iter().filter(|x| x.is_object()).collect();
                if list.is_empty() {
                    return None;
                }
                list.sort_by(|x, y| {
                    let t = |m: &Value| m.obj("At").map(|at| marker_ms(at, tempo)).unwrap_or(0.0);
                    t(x).total_cmp(&t(y))
                });
                Some(
                    list.into_iter()
                        .map(|m| MusicMarker {
                            name: m.str_at("Name"),
                            at: m
                                .obj("At")
                                .map(|at| aerie_protocol::proto::BarBeatPosition {
                                    bar: at.i32_or("Bar", 1),
                                    beat: at.i32_or("Beat", 1),
                                    ms: at.f32_or("Ms", 0.0),
                                }),
                        })
                        .collect(),
                )
            });
            let layers = match a.get("Layers").and_then(Value::as_array) {
                Some(l) if l.iter().any(Value::is_object) => {
                    let owner = owner_of(store, key, "Layers");
                    let mut out = Vec::new();
                    for (i, layer) in l.iter().enumerate().filter(|(_, x)| x.is_object()) {
                        let c = layer
                            .get("Container")
                            .and_then(|c| reference(c, &owner, &format!("Layers_{i}_Container")));
                        out.push(LayerPlacement {
                            container_index: container_index(ctx, c.as_deref())?,
                            name: layer.str_at("Name"),
                            clip_start: layer.obj("ClipStart").map(bar_beat),
                            state_bindings: audio_states::bindings(ctx, layer)?,
                        });
                    }
                    Some(out)
                }
                _ => None,
            };
            MusicContainer::SegmentMusicContainer(Box::new(base!(SegmentMusicContainer, ctx, a, {
                layers,
                entry_marker: a.obj("EntryMarker").map(bar_beat),
                exit_marker: a.obj("ExitMarker").map(bar_beat),
                markers,
            })))
        }
        _ => MusicContainer::SingleTrackMusicContainer(Box::new(
            base!(SingleTrackMusicContainer, ctx, a, {
                track: a.str_at("Track"),
            }),
        )),
    })
}

pub fn build_containers(ctx: &Ctx) -> Result<UpdateMusicContainers> {
    let store = ctx.store(CONTAINERS)?;
    let order = ctx.order(CONTAINERS, CONTAINERS_BUILTIN)?;
    let mut list = Vec::new();
    for (i, k) in order.iter().enumerate() {
        let c = if store.own.contains_key(k) {
            container(ctx, &store, k)?
        } else {
            container(ctx, &Store::empty(CONTAINERS), k)?
        };
        list.push((i as i32, c));
    }
    Ok(UpdateMusicContainers {
        r#type: super::INIT,
        max_id: order.len() as i32,
        music_containers: Some(list),
    })
}

pub fn build_stingers(ctx: &Ctx) -> Result<UpdateStingers> {
    let store = ctx.store(STINGERS)?;
    let order = ctx.order(STINGERS, STINGERS_BUILTIN)?;
    let mut list = Vec::new();
    for (i, k) in order.iter().enumerate() {
        let a = if store.own.contains_key(k) {
            store.resolved(k, "Stinger")
        } else {
            NULL.clone()
        };
        let c = a
            .get("MusicContainer")
            .and_then(|c| reference(c, &owner_of(&store, k, "MusicContainer"), "MusicContainer"));
        list.push((
            i as i32,
            Stinger {
                id: Some(k.clone()),
                music_container_index: container_index(ctx, c.as_deref())?,
                max_instance: a.i32_or("MaxInstance", 1),
                priority: a.i32_or("Priority", 0) as i16,
                exclusion_group: a.str_at("ExclusionGroup"),
                audio_category_index: ctx.index_or(
                    audio_states::CATEGORIES,
                    audio_states::CATEGORIES_BUILTIN,
                    a.str_at("AudioCategory").as_deref(),
                    0,
                )?,
                volume: a
                    .get("Volume")
                    .and_then(Value::as_f64)
                    .map(|d| db_gain(d as f32))
                    .unwrap_or(1.0),
                ducking_rules: audio_states::ducking(ctx, &a)?,
            },
        ));
    }
    Ok(UpdateStingers {
        r#type: super::INIT,
        max_id: order.len() as i32,
        stingers: Some(list),
    })
}

/// A transition's `From` / `To`: the matched container indexes, sorted, and
/// the rule's specificity.
fn matched(ctx: &Ctx, m: Option<&Value>) -> Result<(Option<Vec<i32>>, i32)> {
    let Some(m) = m.filter(|m| m.is_object()) else {
        return Ok((None, 0));
    };
    if let Some(c) = m.str_at("Container") {
        let store = ctx.store(CONTAINERS)?;
        let known =
            store.own.keys().any(|k| k.eq_ignore_ascii_case(&c)) || c.eq_ignore_ascii_case("Empty");
        let v = if known {
            vec![container_index(ctx, Some(&c))?]
        } else {
            Vec::new()
        };
        return Ok((Some(v), 2));
    }
    let pattern = match m.get("TagPattern") {
        Some(p @ Value::Object(_)) => Some(p.clone()),
        Some(Value::String(s)) => {
            let tp = ctx.store(super::tag_patterns::PATH)?;
            tp.own
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(s))
                .map(|(_, v)| v.clone())
        }
        _ => return Ok((None, 0)),
    };
    let Some(pattern) = pattern else {
        return Ok((Some(Vec::new()), 1));
    };
    let store = ctx.store(CONTAINERS)?;
    let mut v = Vec::new();
    for k in store.own.keys() {
        let tags = tag_names(&store, k);
        if !tags.is_empty() && pattern_matches(&pattern, &tags) {
            v.push(container_index(ctx, Some(k))?);
        }
    }
    v.sort();
    Ok((Some(v), 1))
}

pub fn build_transitions(ctx: &Ctx) -> Result<UpdateMusicTransitions> {
    let store = ctx.store(TRANSITIONS)?;
    let order = ctx.order(TRANSITIONS, TRANSITIONS_BUILTIN)?;
    let eq_sine = FadeCurve::from_name("EqualPowerSine").unwrap_or_default();
    let mut list = Vec::new();
    for (i, k) in order.iter().enumerate() {
        let a = if store.own.contains_key(k) {
            store.resolved(k, "MusicTransition")
        } else {
            NULL.clone()
        };
        let (from, fs) = matched(ctx, a.get("From"))?;
        let (to, ts) = matched(ctx, a.get("To"))?;
        list.push((
            i as i32,
            MusicTransition {
                id: Some(k.clone()),
                from_container_indices: from,
                to_container_indices: to,
                exit_at: a.obj("ExitAt").map(music_sync),
                align: enum_or(
                    &a,
                    "Align",
                    DestinationAlign::from_name,
                    DestinationAlign::ENTRY_MARKER,
                ),
                align_marker_name: a.str_at("AlignMarker"),
                fade_out_duration: a.obj("FadeOutDuration").map(bar_beat),
                fade_out_curve: enum_or(&a, "FadeOutCurve", FadeCurve::from_name, eq_sine),
                fade_out_delay: a.obj("FadeOutDelay").map(bar_beat),
                fade_in_duration: a.obj("FadeInDuration").map(bar_beat),
                fade_in_curve: enum_or(&a, "FadeInCurve", FadeCurve::from_name, eq_sine),
                play_post_exit: a.bool_or("PlayPostExit", false),
                post_event: a.str_at("PostEvent"),
                priority: a.i32_or("Priority", 0),
                specificity: fs * 3 + ts,
            },
        ));
    }
    Ok(UpdateMusicTransitions {
        r#type: super::INIT,
        max_id: order.len() as i32,
        music_transitions: Some(list),
    })
}

/// A container with its index fields cleared, as a matching signature.
fn signature(c: &MusicContainer) -> String {
    let mut c = c.clone();
    fn clear(children: &mut Option<Vec<i32>>, stingers: &mut Option<Vec<StingerBinding>>) {
        if let Some(v) = children {
            v.iter_mut().for_each(|x| *x = 0);
        }
        if let Some(v) = stingers {
            v.iter_mut().for_each(|s| s.stinger_index = 0);
        }
    }
    match &mut c {
        MusicContainer::SingleTrackMusicContainer(m) => clear(&mut None, &mut m.stingers),
        MusicContainer::RandomMusicContainer(m) => clear(&mut m.children, &mut m.stingers),
        MusicContainer::SequenceMusicContainer(m) => clear(&mut m.children, &mut m.stingers),
        MusicContainer::HorizontalMusicContainer(m) => clear(&mut m.children, &mut m.stingers),
        MusicContainer::SegmentMusicContainer(m) => {
            clear(&mut None, &mut m.stingers);
            if let Some(l) = &mut m.layers {
                l.iter_mut().for_each(|x| x.container_index = 0);
            }
        }
    }
    format!("{c:?}")
}

/// Compare runs only: music containers carry no id, and the official order
/// of the contained ones written in other folders (ambience effects,
/// stingers) interleaves with no stable rule. This pins each captured index
/// to the Aerie container with the same content (index fields aside); equal
/// contents are paired in order.
pub fn pin_containers(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    use aerie_protocol::wire::Wire;
    let (cap, _) = <UpdateMusicContainers as Wire>::decode(captured, 0)?;
    let built = build_containers(ctx)?;
    let order = ctx.order(CONTAINERS, CONTAINERS_BUILTIN)?;
    let mut ours: std::collections::HashMap<String, Vec<String>> = Default::default();
    for (i, c) in built.music_containers.unwrap_or_default() {
        ours.entry(signature(&c))
            .or_default()
            .push(order[i as usize].clone());
    }
    let mut cap = cap.music_containers.unwrap_or_default();
    cap.sort_by_key(|(i, _)| *i);
    let mut pairs = Vec::new();
    let mut used: std::collections::HashMap<String, usize> = Default::default();
    for (i, c) in &cap {
        let sig = signature(c);
        if let Some(keys) = ours.get(&sig) {
            let n = used.entry(sig).or_default();
            if let Some(k) = keys.get(*n) {
                pairs.push((k.clone(), *i));
                *n += 1;
            }
        }
    }
    ctx.pin(CONTAINERS, pairs);
    Ok(())
}
