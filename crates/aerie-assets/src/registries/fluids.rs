//! `UpdateFluids` (83) from `Item/Block/Fluids`, indexed, built-in `Empty`
//! and `Unknown` first (spec 07). Inheritance follows the schema.
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | key | `id` | |
//! | `MaxFluidLevel` | `max_fluid_level` | 8 |
//! | `Textures` (block textures) | `cube_textures` | one `BlockTextures/Unknown.png` cube |
//! | `RequiresAlphaBlending` | same | true |
//! | `Opacity` | same | `Solid` |
//! | `Effect` (shader names) | `shader_effect` | `[None]` |
//! | `Light` `{Color, Radius}` | `light` | absent |
//! | `Particles` (model particles) | same | absent when empty |
//! | `DrawType` | `draw_type` | `Liquid` |
//! | `FluidFXId` | `fluid_fx_index` | 0 (`Empty`) |
//! | `BlockSoundSetId` | `block_sound_set_index` | 0 (`EMPTY`) |
//! | `BlockParticleSetId`, `ParticleColor` | same | absent |
//! | `Tags` | `tag_indexes` | absent for the built-in entries |
//!
//! Block textures (shared with block types): `All` (every face), `Sides`
//! (north, south, west, east), `UpDown`, then `Up`, `Down`, `North`, `South`,
//! `West`, `East`, applied in the order written; faces map to top, bottom,
//! back (north), front (south), left (west), right (east). `Weight` (1) is
//! sent divided by the sum of the list's weights. Light: `Color` `#rgb` (each
//! digit 0-15) or `#rrggbb` (each byte / 17), `Radius` (0).

use aerie_protocol::proto::{
    BlockTextures, ColorLight, Fluid, FluidDrawType, Opacity, ShaderType, UpdateFluids,
};
use aerie_protocol::wire::Wire;
use serde_json::Value;

use super::{materials, model_particles, sound_sets};
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;
use crate::{Result, store::Store, tags};

pub const PATH: &str = "Item/Block/Fluids";
pub const BUILTIN: &[&str] = &["Empty", "Unknown"];

pub fn unknown_textures() -> Vec<BlockTextures> {
    let t = Some("BlockTextures/Unknown.png".to_owned());
    vec![BlockTextures {
        top: t.clone(),
        bottom: t.clone(),
        front: t.clone(),
        back: t.clone(),
        left: t.clone(),
        right: t,
        weight: 1.0,
    }]
}

/// A list of block textures; `None` when absent or empty.
pub fn block_textures(v: Option<&Value>) -> Option<Vec<BlockTextures>> {
    let list = v?.as_array().filter(|l| !l.is_empty())?;
    let total: f32 = list.iter().map(|t| t.i32_or("Weight", 1) as f32).sum();
    Some(
        list.iter()
            .map(|t| {
                let mut faces: [Option<String>; 6] = Default::default(); // up down north south west east
                if let Some(m) = t.as_object() {
                    for (k, v) in m {
                        let Some(s) = v.as_str().map(str::to_owned) else {
                            continue;
                        };
                        let set: &[usize] = match k.as_str() {
                            "All" => &[0, 1, 2, 3, 4, 5],
                            "Sides" => &[2, 3, 4, 5],
                            "UpDown" => &[0, 1],
                            "Up" => &[0],
                            "Down" => &[1],
                            "North" => &[2],
                            "South" => &[3],
                            "West" => &[4],
                            "East" => &[5],
                            _ => &[],
                        };
                        for &i in set {
                            faces[i] = Some(s.clone());
                        }
                    }
                }
                let w = t.i32_or("Weight", 1) as f32;
                let [up, down, north, south, west, east] = faces;
                BlockTextures {
                    top: up,
                    bottom: down,
                    front: south,
                    back: north,
                    left: west,
                    right: east,
                    weight: if total > 0.0 { w / total } else { total },
                }
            })
            .collect(),
    )
}

/// `{Color, Radius}` light.
pub fn light(v: &Value) -> ColorLight {
    let mut l = ColorLight {
        radius: v.i32_or("Radius", 0) as i8,
        ..Default::default()
    };
    if let Some(c) = v.str_at("Color") {
        let hex = |s: &str| i32::from_str_radix(s, 16).unwrap_or(0);
        if c.len() == 4 {
            l.red = hex(&c[1..2]) as i8;
            l.green = hex(&c[2..3]) as i8;
            l.blue = hex(&c[3..4]) as i8;
        } else if c.len() >= 7 {
            l.red = (hex(&c[1..3]) / 17) as i8;
            l.green = (hex(&c[3..5]) / 17) as i8;
            l.blue = (hex(&c[5..7]) / 17) as i8;
        }
    }
    l
}

fn fluid(ctx: &Ctx, store: &Store, k: &str) -> Result<Fluid> {
    let builtin = !store.own.contains_key(k);
    let a = if builtin {
        Value::Null
    } else {
        store.resolved(k, "Fluid")
    };
    let fx = a.str_at("FluidFXId").unwrap_or_else(|| "Empty".into());
    let sounds = a
        .str_at("BlockSoundSetId")
        .unwrap_or_else(|| "EMPTY".into());
    let particles = a
        .get("Particles")
        .and_then(Value::as_array)
        .filter(|p| !p.is_empty())
        .map(|p| p.iter().map(model_particles::one).collect());
    let shader = a
        .get("Effect")
        .and_then(Value::as_array)
        .filter(|e| !e.is_empty())
        .map(|e| {
            e.iter()
                .filter_map(|s| s.as_str().and_then(ShaderType::from_name))
                .collect()
        })
        .unwrap_or_else(|| vec![ShaderType::NONE]);
    Ok(Fluid {
        id: Some(k.to_owned()),
        max_fluid_level: a.i32_or("MaxFluidLevel", 8),
        cube_textures: Some(block_textures(a.get("Textures")).unwrap_or_else(unknown_textures)),
        requires_alpha_blending: a.bool_or("RequiresAlphaBlending", true),
        opacity: enum_or(&a, "Opacity", Opacity::from_name, Opacity::SOLID),
        shader_effect: Some(shader),
        light: a.obj("Light").map(light),
        particles,
        draw_type: enum_or(
            &a,
            "DrawType",
            FluidDrawType::from_name,
            FluidDrawType::LIQUID,
        ),
        fluid_fx_index: if fx == "Empty" {
            0
        } else {
            ctx.index_or(
                materials::FLUID_FX,
                materials::FLUID_FX_BUILTIN,
                Some(&fx),
                i32::MIN,
            )?
        },
        block_sound_set_index: if sounds == "EMPTY" {
            0
        } else {
            ctx.index_or(
                sound_sets::BLOCK,
                sound_sets::EMPTY,
                Some(&sounds),
                i32::MIN,
            )?
        },
        block_particle_set_id: a.str_at("BlockParticleSetId"),
        particle_color: a.color("ParticleColor"),
        tag_indexes: (!builtin).then(|| tags::indexes(ctx, store, k)),
    })
}

pub fn build(ctx: &Ctx) -> Result<UpdateFluids> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    let mut list = Vec::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        list.push((i, fluid(ctx, &store, k)?));
    }
    Ok(UpdateFluids {
        r#type: super::INIT,
        max_id: order.len() as i32,
        fluids: Some(list),
    })
}

/// Compare runs: learns tag indexes from the captured fluids.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    let (cap, _) = <UpdateFluids as Wire>::decode(captured, 0)?;
    let store = ctx.store(PATH)?;
    let obs: Vec<(Vec<String>, Vec<i32>)> = cap
        .fluids
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, f)| {
            let id = f.id?;
            let k = store.own.keys().find(|k| k.eq_ignore_ascii_case(&id))?;
            Some((tags::of(&store, k).expanded, f.tag_indexes?))
        })
        .collect();
    tags::learn(ctx, &obs);
    Ok(())
}
