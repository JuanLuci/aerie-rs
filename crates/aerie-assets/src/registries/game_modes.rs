//! Game mode types for the server (spec 30), from `Server/Entity/GameMode`.
//! Clients are sent nothing of them, so this is a server table: one line per
//! type, `key<TAB>{…}`, with the fields the type sets (the unset ones are
//! left out, because "unset" means "leave the player as it is"):
//!
//! | JSON | Table |
//! | --- | --- |
//! | `InteractionsOnEnter` | `OnEnter`: the name of the root interaction (an inline one is named `*Key_InteractionsOnEnter`) |
//! | `EntityEffectId` | `Effect`: the effect's index (dropped when unknown) |
//! | `InteractionOverrides` | `Overrides`: interaction type name → root name (inline: `*Key_InteractionOverrides_Type`) |
//! | `MovementConfigId`, `VoiceChannel`, `LockedCameraView` | same |
//! | `Spectator`, `Flying`, `NoClip`, `Invulnerable`, `Intangible`, `ItemDrops`, `PreventInventoryAccess`, `PreventEmotes`, `TriggerBlocks`, `OverrideAllInteractions` | same |

use serde_json::{Map, Value};

use super::entity_effects;
use crate::Result;
use crate::ctx::Ctx;
use crate::values::Get;

pub const PATH: &str = "Entity/GameMode";

/// The name of the root interaction a field holds: the name itself, or the
/// generated one of an inline definition.
fn root_name(v: &Value, inline: &str) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => Some(format!("*{inline}")),
        _ => None,
    }
}

pub fn table(ctx: &Ctx) -> Result<String> {
    let store = ctx.store(PATH)?;
    let mut keys: Vec<&String> = store.own.keys().collect();
    keys.sort();
    let mut out = String::new();
    for k in keys {
        let a = store.resolved(k, "GameModeType");
        let mut o = Map::new();
        if let Some(r) = a
            .get("InteractionsOnEnter")
            .and_then(|v| root_name(v, &format!("{k}_InteractionsOnEnter")))
        {
            o.insert("OnEnter".into(), r.into());
        }
        if let Some(name) = a.str_at("EntityEffectId") {
            let i = ctx.index_or(entity_effects::PATH, &[], Some(&name), i32::MIN)?;
            if i != i32::MIN {
                o.insert("Effect".into(), i.into());
            }
        }
        if let Some(m) = a.obj("InteractionOverrides").and_then(Value::as_object) {
            let mut list = Map::new();
            for (ty, v) in m {
                if let Some(r) = root_name(v, &format!("{k}_InteractionOverrides_{ty}")) {
                    list.insert(ty.clone(), r.into());
                }
            }
            if !list.is_empty() {
                o.insert("Overrides".into(), Value::Object(list));
            }
        }
        for f in [
            "MovementConfigId",
            "VoiceChannel",
            "LockedCameraView",
            "Spectator",
            "Flying",
            "NoClip",
            "Invulnerable",
            "Intangible",
            "ItemDrops",
            "PreventInventoryAccess",
            "PreventEmotes",
            "TriggerBlocks",
            "OverrideAllInteractions",
        ] {
            if let Some(v) = a.get(f).filter(|v| !v.is_null()) {
                o.insert(f.into(), v.clone());
            }
        }
        out += &format!("{k}\t{}\n", Value::Object(o));
    }
    Ok(out)
}
