//! `UpdateEnvironments` (61) from `Environments`, indexed, built-in `Unknown`
//! (water tint `#0a3355`, no tags) first (spec 07). Inheritance follows the
//! schema.
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | key | `id` | |
//! | `WaterTint` (color) | `water_tint` | absent |
//! | `FluidParticles` `{fluid FX: {SystemId, Color, Scale (1)}}` | `fluid_particles` (fluid FX index; unknown ones dropped) | absent when empty |
//! | `Tags` | `tag_indexes` (spec 07, "Tags") | |
//!
//! `rebuild_map_geometry` is true during setup. The rest of an environment (weather
//! forecasts, spawn density, block modification) stays on the server.

use aerie_protocol::proto::{Color, FluidParticle, UpdateEnvironments, WorldEnvironment};
use aerie_protocol::wire::Wire;
use serde_json::Value;

use super::materials;
use crate::values::Get;
use crate::{Result, ctx::Ctx, tags};

pub const PATH: &str = "Environments";
pub const BUILTIN: &[&str] = &["Unknown"];

fn environment(ctx: &Ctx, store: &crate::store::Store, k: &str) -> Result<WorldEnvironment> {
    if !store.own.contains_key(k) {
        return Ok(WorldEnvironment {
            id: Some(k.to_owned()),
            water_tint: Some(Color {
                red: 10,
                green: 51,
                blue: 85,
            }),
            fluid_particles: None,
            tag_indexes: None,
        });
    }
    let a = store.resolved(k, "Environment");
    let fluid_particles = match a.obj("FluidParticles").and_then(Value::as_object) {
        Some(m) if !m.is_empty() => {
            let mut out = Vec::new();
            for (fx, p) in m {
                let i = ctx.index_or(
                    materials::FLUID_FX,
                    materials::FLUID_FX_BUILTIN,
                    Some(fx),
                    i32::MIN,
                )?;
                if i != i32::MIN {
                    out.push((
                        i,
                        FluidParticle {
                            system_id: p.str_at("SystemId"),
                            color: p.color("Color"),
                            scale: p.f32_or("Scale", 1.0),
                        },
                    ));
                }
            }
            Some(out)
        }
        _ => None,
    };
    Ok(WorldEnvironment {
        id: Some(k.to_owned()),
        water_tint: a.color("WaterTint"),
        fluid_particles,
        tag_indexes: Some(tags::indexes(ctx, store, k)),
    })
}

pub fn build(ctx: &Ctx) -> Result<UpdateEnvironments> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    let mut list = Vec::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        list.push((i, environment(ctx, &store, k)?));
    }
    Ok(UpdateEnvironments {
        r#type: super::INIT,
        max_id: order.len() as i32,
        environments: Some(list),
        rebuild_map_geometry: true,
    })
}

/// Compare runs: learns tag indexes from the captured environments.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    let (cap, _) = <UpdateEnvironments as Wire>::decode(captured, 0)?;
    let store = ctx.store(PATH)?;
    let obs: Vec<(Vec<String>, Vec<i32>)> = cap
        .environments
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, e)| {
            let id = e.id?;
            let k = store.own.keys().find(|k| k.eq_ignore_ascii_case(&id))?;
            Some((tags::of(&store, k).expanded, e.tag_indexes?))
        })
        .collect();
    tags::learn(ctx, &obs);
    Ok(())
}
