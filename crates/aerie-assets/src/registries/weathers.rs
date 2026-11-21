//! `UpdateWeathers` (47) from `Weathers`, indexed, built-in `Unknown` first
//! (spec 07). Inheritance follows the schema.
//!
//! Time curves are lists of `{Hour, Value}` (`…Multipliers`, `…Scales`,
//! `…Falloffs`, `…Densities`) or `{Hour, Color}` (colors, with alpha for sky,
//! glow, moon and screen effect colors), sent as hour → value maps; empty
//! lists are absent. `Moons` are `{Day, Texture}` (day → texture). `Clouds`:
//! `Texture`, `Colors` and `Speeds` curves. `FogDistance` `[near, far]`
//! (-96, 1024) → `fog`. `FogOptions`: `IgnoreFogLimits`,
//! `EffectiveViewDistanceMultiplier` (1), `FogHeightCameraOffset` (0),
//! `FogHeightCameraFixed` (sets `fog_height_camera_overriden` when given).
//! `Particle`: `SystemId`, `Color`, `Scale`, `OvergroundOnly`,
//! `PositionOffsetMultiplier` (0). `Stars`, `ScreenEffect`; `Tags` →
//! `tag_indexes` (absent for `Unknown`).

use aerie_protocol::proto::{
    Cloud, Color, ColorAlpha, FogOptions, NearFar, UpdateWeathers, Weather, WeatherParticle,
};
use aerie_protocol::wire::Wire;
use serde_json::Value;

use crate::values::Get;
use crate::{Result, ctx::Ctx, store::Store, tags};

pub const PATH: &str = "Weathers";
pub const BUILTIN: &[&str] = &["Unknown"];

fn list(v: &Value, key: &str) -> Option<Vec<Value>> {
    v.get(key)?.as_array().filter(|a| !a.is_empty()).cloned()
}

fn floats(v: &Value, key: &str) -> Option<Vec<(f32, f32)>> {
    list(v, key).map(|l| {
        l.iter()
            .map(|e| (e.f32_or("Hour", 0.0), e.f32_or("Value", 0.0)))
            .collect()
    })
}

fn colors(v: &Value, key: &str) -> Option<Vec<(f32, Color)>> {
    list(v, key).map(|l| {
        l.iter()
            .map(|e| (e.f32_or("Hour", 0.0), e.color("Color").unwrap_or_default()))
            .collect()
    })
}

fn colors_alpha(v: &Value, key: &str) -> Option<Vec<(f32, ColorAlpha)>> {
    list(v, key).map(|l| {
        l.iter()
            .map(|e| {
                (
                    e.f32_or("Hour", 0.0),
                    e.color_alpha("Color").unwrap_or_default(),
                )
            })
            .collect()
    })
}

fn weather(ctx: &Ctx, store: &Store, k: &str) -> Weather {
    let fog = |a: &Value| {
        let d = a.get("FogDistance").and_then(Value::as_array);
        let at = |i: usize, def: f32| {
            d.and_then(|d| d.get(i))
                .and_then(Value::as_f64)
                .map_or(def, |x| x as f32)
        };
        NearFar {
            near: at(0, -96.0),
            far: at(1, 1024.0),
        }
    };
    if !store.own.contains_key(k) {
        return Weather {
            id: Some(k.to_owned()),
            fog: Some(fog(&Value::Null)),
            ..Default::default()
        };
    }
    let a = store.resolved(k, "Weather");
    Weather {
        id: Some(k.to_owned()),
        tag_indexes: Some(tags::indexes(ctx, store, k)),
        stars: a.str_at("Stars"),
        moons: list(&a, "Moons").map(|l| {
            l.iter()
                .map(|m| (m.i32_or("Day", 0), m.str_at("Texture").unwrap_or_default()))
                .collect()
        }),
        clouds: list(&a, "Clouds").map(|l| {
            l.iter()
                .map(|c| Cloud {
                    texture: c.str_at("Texture"),
                    speeds: floats(c, "Speeds").or(Some(Vec::new())),
                    colors: colors_alpha(c, "Colors").or(Some(Vec::new())),
                })
                .collect()
        }),
        sunlight_damping_multiplier: floats(&a, "SunlightDampingMultipliers"),
        sunlight_colors: colors(&a, "SunlightColors"),
        sky_top_colors: colors_alpha(&a, "SkyTopColors"),
        sky_bottom_colors: colors_alpha(&a, "SkyBottomColors"),
        sky_sunset_colors: colors_alpha(&a, "SkySunsetColors"),
        sun_colors: colors(&a, "SunColors"),
        sun_scales: floats(&a, "SunScales"),
        sun_glow_colors: colors_alpha(&a, "SunGlowColors"),
        moon_colors: colors_alpha(&a, "MoonColors"),
        moon_scales: floats(&a, "MoonScales"),
        moon_glow_colors: colors_alpha(&a, "MoonGlowColors"),
        fog_colors: colors(&a, "FogColors"),
        fog_height_falloffs: floats(&a, "FogHeightFalloffs"),
        fog_densities: floats(&a, "FogDensities"),
        screen_effect: a.str_at("ScreenEffect"),
        screen_effect_colors: colors_alpha(&a, "ScreenEffectColors"),
        color_filters: colors(&a, "ColorFilters"),
        water_tints: colors(&a, "WaterTints"),
        particle: a.obj("Particle").map(|p| WeatherParticle {
            system_id: p.str_at("SystemId"),
            color: p.color("Color"),
            scale: p.f32_or("Scale", 0.0),
            is_overground_only: p.bool_or("OvergroundOnly", false),
            position_offset_multiplier: p.f32_or("PositionOffsetMultiplier", 0.0),
        }),
        fog: Some(fog(&a)),
        fog_options: a.obj("FogOptions").map(|f| {
            let fixed = f.get("FogHeightCameraFixed").and_then(Value::as_f64);
            FogOptions {
                ignore_fog_limits: f.bool_or("IgnoreFogLimits", false),
                effective_view_distance_multiplier: f
                    .f32_or("EffectiveViewDistanceMultiplier", 1.0),
                fog_far_view_distance: 0.0,
                fog_height_camera_offset: f.f32_or("FogHeightCameraOffset", 0.0),
                fog_height_camera_overriden: fixed.is_some(),
                fog_height_camera_fixed: fixed.unwrap_or(0.0) as f32,
            }
        }),
    }
}

pub fn build(ctx: &Ctx) -> Result<UpdateWeathers> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    let list = order
        .iter()
        .enumerate()
        .map(|(n, k)| {
            let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
            (i, weather(ctx, &store, k))
        })
        .collect();
    Ok(UpdateWeathers {
        r#type: super::INIT,
        max_id: order.len() as i32,
        weathers: Some(list),
    })
}

/// Compare runs: learns tag indexes from the captured weathers.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    let (cap, _) = <UpdateWeathers as Wire>::decode(captured, 0)?;
    let store = ctx.store(PATH)?;
    let obs: Vec<(Vec<String>, Vec<i32>)> = cap
        .weathers
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, w)| {
            let id = w.id?;
            let k = store.own.keys().find(|k| k.eq_ignore_ascii_case(&id))?;
            Some((tags::of(&store, k).expanded, w.tag_indexes?))
        })
        .collect();
    tags::learn(ctx, &obs);
    Ok(())
}
