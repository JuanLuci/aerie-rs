//! `UpdateItemReticles` (57) from `Item/Reticles`, indexed, built-in
//! `Default` first (spec 07). Inheritance follows the schema.
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | `Base` (texture list) | `base` | absent (`Default`: `UI/Reticles/Default.png`) |
//! | `ServerEvents` `{event: reticle}` | `server_events`, keyed by the event name's **tag index** | absent |
//! | `ClientEvents` `{event: reticle}` | `client_events`, keyed by `ItemReticleClientEvent` | absent |
//!
//! Reticle: `HideBase` (false), `Parts` (texture list), `Duration` (server
//! events only: 0.25; client events send 0). The packet carries no id.

use aerie_protocol::proto::{
    ItemReticle, ItemReticleClientEvent, ItemReticleConfig, UpdateItemReticles,
};
use aerie_protocol::wire::Wire;
use serde_json::Value;

use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub const PATH: &str = "Item/Reticles";
pub const BUILTIN: &[&str] = &["Default"];

fn strings(v: &Value, key: &str) -> Option<Vec<String>> {
    v.get(key)?.as_array().map(|a| {
        a.iter()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect()
    })
}

fn reticle(v: &Value, duration: Option<f32>) -> ItemReticle {
    ItemReticle {
        hide_base: v.bool_or("HideBase", false),
        parts: strings(v, "Parts"),
        duration: duration.map(|d| v.f32_or("Duration", d)).unwrap_or(0.0),
    }
}

fn config(ctx: &Ctx, a: &Value) -> ItemReticleConfig {
    ItemReticleConfig {
        id: None,
        base: strings(a, "Base"),
        server_events: a.obj("ServerEvents").and_then(Value::as_object).map(|m| {
            m.iter()
                .map(|(e, r)| (ctx.tag(e), reticle(r, Some(0.25))))
                .collect()
        }),
        client_events: a.obj("ClientEvents").and_then(Value::as_object).map(|m| {
            m.iter()
                .filter_map(|(e, r)| {
                    Some((ItemReticleClientEvent::from_name(e)?, reticle(r, None)))
                })
                .collect()
        }),
    }
}

pub fn build(ctx: &Ctx) -> Result<UpdateItemReticles> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let list = order
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let c = match store.own.get(k) {
                Some(_) => config(ctx, &store.resolved(k, "ItemReticleConfig")),
                None => ItemReticleConfig {
                    base: Some(vec!["UI/Reticles/Default.png".into()]),
                    ..Default::default()
                },
            };
            (i as i32, c)
        })
        .collect();
    Ok(UpdateItemReticles {
        r#type: super::INIT,
        max_id: order.len() as i32,
        item_reticle_configs: Some(list),
    })
}

/// Learns the tag indexes of server event names from a capture: config i of
/// the capture is Aerie's config i, and each event's parts tell which one it
/// is.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    let (cap, _) = <UpdateItemReticles as Wire>::decode(captured, 0)?;
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    for (i, c) in cap.item_reticle_configs.unwrap_or_default() {
        let Some(k) = order.get(i as usize) else {
            continue;
        };
        if !store.own.contains_key(k) {
            continue;
        }
        let a = store.resolved(k, "ItemReticleConfig");
        let Some(events) = a.obj("ServerEvents").and_then(Value::as_object) else {
            continue;
        };
        for (tag, r) in c.server_events.unwrap_or_default() {
            let named = events
                .iter()
                .filter(|(_, v)| strings(v, "Parts") == r.parts)
                .map(|(e, _)| e)
                .collect::<Vec<_>>();
            if let [e] = named.as_slice() {
                ctx.pin_tag(e, tag);
            }
        }
    }
    Ok(())
}
