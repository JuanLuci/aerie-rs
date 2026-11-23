//! What breaking a block takes and gives (spec 12), from the assets:
//!
//! - a block's `Gathering`: `Breaking` (`GatherType`, `Quality` 0,
//!   `ItemId`, `DropList`, `Quantity` 1), `Soft` (`ItemId`, `DropList`,
//!   `IsWeaponBreakable` true), `Harvest` (`ItemId`, `DropList`),
//!   `UseDefaultDropWhenPlaced`;
//! - an item's `Tool`: `Specs` (`GatherType`, `Power`, `Quality`,
//!   `IsIncorrect`, `HitSoundLayer`) in order, `HitSoundLayer`,
//!   `IncorrectMaterialSoundLayer`, `DurabilityLossBlockTypes`; whether it is
//!   a weapon or a builder tool; `MaxDurability`, `DurabilityLossOnHit`;
//! - bare hands: `Item/Unarmed/Gathering/<GatherType>`;
//! - the gameplay config's `Gathering.UnbreakableBlock` and
//!   `Gathering.IncorrectTool` effects (particle system, sound) and
//!   `ItemDurability.BrokenPenalties.Tool`.
//!
//! Text lines for the start-up cache (`-` for none, sounds as event
//! indexes):
//! `B key gather quality item list qty soft softItem softList softWeapon
//! harvest harvestItem harvestList useDefault`,
//! `T item gather power quality incorrect hitSound` (every spec, in order),
//! `I item weapon builder tool hitSound incorrectSound maxDurability
//! lossOnHit`, `L item loss sets types` (comma lists),
//! `U gather power quality hitSound`, `E kind particle sound`,
//! `X tool penalty`; a block's `Gathering.Tools` as `G block tool state
//! item list`; and the break tools an item's interactions reach (spec 12:
//! the `BreakBlock` interactions under each interaction type, with the
//! item's `InteractionVars` in place of the defaults they replace) as
//! `K item type tool matchTool`.

use std::collections::{BTreeSet, HashSet};

use serde_json::Value;

use super::music::{owner_of, reference};
use crate::Result;
use crate::ctx::Ctx;
use crate::values::Get;

pub const UNARMED: &str = "Item/Unarmed/Gathering";
const GAMEPLAY: &str = "GameplayConfigs";

fn or_dash(s: Option<String>) -> String {
    s.filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
        .unwrap_or_else(|| "-".into())
}

fn sound(ctx: &Ctx, v: Option<String>) -> Result<i32> {
    Ok(match v {
        Some(n) => super::sound_events::index(ctx, Some(&n))?.max(0),
        None => 0,
    })
}

fn list(v: Option<Vec<String>>) -> String {
    match v {
        Some(v) if !v.is_empty() => v.join(","),
        _ => "-".into(),
    }
}

/// Walks interactions for the `BreakBlock`s they can run: (tool, match).
struct Walk<'a> {
    interactions: &'a crate::store::Store,
    roots: &'a crate::store::Store,
    vars: Option<&'a serde_json::Map<String, Value>>,
    seen: HashSet<String>,
    found: BTreeSet<(String, bool)>,
}

impl Walk<'_> {
    fn key(&mut self, k: &str) {
        if !self.seen.insert(k.to_owned()) {
            return;
        }
        if self.interactions.assets.contains_key(k) {
            let v = self.interactions.resolved(k, "Interaction");
            self.value(&v);
        } else if self.roots.assets.contains_key(k) {
            let v = self.roots.resolved(k, "RootInteraction");
            self.value(&v);
        }
    }

    fn value(&mut self, v: &Value) {
        match v {
            Value::String(k) => self.key(k),
            Value::Array(a) => a.iter().for_each(|x| self.value(x)),
            Value::Object(o) => {
                let ty = o.get("Type").and_then(Value::as_str);
                if ty == Some("BreakBlock") {
                    let tool = o.get("Tool").and_then(Value::as_str).unwrap_or("-");
                    let m = o.get("MatchTool").and_then(Value::as_bool).unwrap_or(false);
                    self.found.insert((tool.to_owned(), m));
                }
                if let Some(p) = o.get("Parent").and_then(Value::as_str) {
                    self.key(p);
                }
                for (k, x) in o {
                    // Names that are not references.
                    if matches!(k.as_str(), "Parent" | "Type" | "Var" | "$Comment") {
                        continue;
                    }
                    if ty == Some("Replace") && k == "DefaultValue" {
                        let var = o.get("Var").and_then(Value::as_str);
                        if let Some(over) = var.and_then(|n| self.vars?.get(n)) {
                            let over = over.clone();
                            self.value(&over);
                            continue;
                        }
                    }
                    self.value(x);
                }
            }
            _ => {}
        }
    }
}

pub fn table(ctx: &Ctx) -> Result<String> {
    let mut out = String::new();
    let blocks = ctx.store(super::blocks::PATH)?;
    let mut keys: Vec<&String> = blocks.own.keys().collect();
    keys.sort();
    for k in keys {
        let a = blocks.resolved(k, "BlockType");
        let Some(g) = a.obj("Gathering") else {
            continue;
        };
        let owner = owner_of(&blocks, k, "Gathering");
        let drop = |x: &Value, part: &str| {
            x.get("DropList")
                .and_then(|d| reference(d, &owner, &format!("Gathering_{part}_DropList")))
        };
        let nul = Value::Null;
        let b = g.obj("Breaking");
        let soft = g.obj("Soft");
        let harvest = g.obj("Harvest");
        if b.is_none() && soft.is_none() && harvest.is_none() {
            continue;
        }
        let bb = b.unwrap_or(&nul);
        let ss = soft.unwrap_or(&nul);
        let hh = harvest.unwrap_or(&nul);
        for t in g
            .get("Tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(ty) = t.str_at("Type") {
                out += &format!(
                    "G {k} {ty} {} {} {}\n",
                    or_dash(t.str_at("State")),
                    or_dash(t.str_at("ItemId")),
                    or_dash(t.get("DropList").and_then(|d| reference(
                        d,
                        &owner,
                        "Gathering_Tools_DropList"
                    ))),
                );
            }
        }
        out += &format!(
            "B {k} {} {} {} {} {} {} {} {} {} {} {} {} {}\n",
            or_dash(b.and_then(|b| b.str_at("GatherType"))),
            bb.i32_or("Quality", 0),
            or_dash(bb.str_at("ItemId")),
            or_dash(b.and_then(|b| drop(b, "Breaking"))),
            bb.i32_or("Quantity", 1),
            soft.is_some() as u8,
            or_dash(ss.str_at("ItemId")),
            or_dash(soft.and_then(|s| drop(s, "Soft"))),
            ss.bool_or("IsWeaponBreakable", true) as u8,
            harvest.is_some() as u8,
            or_dash(hh.str_at("ItemId")),
            or_dash(harvest.and_then(|h| drop(h, "Harvest"))),
            g.bool_or("UseDefaultDropWhenPlaced", false) as u8,
        );
    }
    let items = ctx.store(super::items::PATH)?;
    let mut keys: Vec<&String> = items.own.keys().collect();
    keys.sort();
    for k in keys {
        let a = items.resolved(k, "Item");
        let tool = a.obj("Tool");
        let weapon = a.obj("Weapon").is_some();
        let builder = a.obj("BuilderTool").is_some();
        if tool.is_none() && !weapon && !builder {
            continue;
        }
        let t = tool.cloned().unwrap_or(Value::Null);
        // A tool that breaks a shape of blocks (spec 12): `Z key kind
        // width height depth centered ox oy oz surface perBlock` (kind 0
        // box, 1 cylinder).
        if let Some(b) = t.obj("BreakShape") {
            let kind = match b.str_at("Id").or_else(|| b.str_at("Type")).as_deref() {
                Some("Cylinder") => 1,
                _ => 0,
            };
            let off = |a: &str| {
                b.obj("Offset")
                    .and_then(|o| o.get(a))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .round() as i64
            };
            out += &format!(
                "Z {k} {kind} {} {} {} {} {} {} {} {} {}\n",
                b.i32_or("Width", 1),
                b.i32_or("Height", 1),
                b.i32_or("Depth", 1),
                b.bool_or("Centered", true) as u8,
                off("X"),
                off("Y"),
                off("Z"),
                (b.str_at("Orientation").as_deref() == Some("Surface")) as u8,
                (t.str_at("BreakShapeDurabilityMode").as_deref() == Some("PerBlock")) as u8,
            );
        }
        out += &format!(
            "I {k} {} {} {} {} {} {} {}\n",
            weapon as u8,
            builder as u8,
            tool.is_some() as u8,
            sound(ctx, t.str_at("HitSoundLayer"))?,
            sound(ctx, t.str_at("IncorrectMaterialSoundLayer"))?,
            a.f64_or("MaxDurability", 0.0),
            a.f64_or("DurabilityLossOnHit", 0.0),
        );
        for s in t
            .get("Specs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(gt) = s.str_at("GatherType") {
                out += &format!(
                    "T {k} {gt} {} {} {} {}\n",
                    s.f32_or("Power", 0.0),
                    s.i32_or("Quality", 0),
                    s.bool_or("IsIncorrect", false) as u8,
                    sound(ctx, s.str_at("HitSoundLayer"))?,
                );
            }
        }
        for d in t
            .get("DurabilityLossBlockTypes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out += &format!(
                "L {k} {} {} {}\n",
                d.f64_or("DurabilityLossOnHit", 0.0),
                list(d.strings("BlockSets")),
                list(d.strings("BlockTypes")),
            );
        }
    }
    let interactions = ctx.store(super::root_interactions::INTERACTIONS)?;
    let roots = ctx.store(super::root_interactions::PATH)?;
    for k in items.own.keys() {
        let a = items.resolved(k, "Item");
        let Some(types) = a.obj("Interactions").and_then(Value::as_object) else {
            continue;
        };
        let vars = a.obj("InteractionVars").and_then(Value::as_object);
        for (ty, root) in types {
            let mut w = Walk {
                interactions: &interactions,
                roots: &roots,
                vars,
                seen: HashSet::new(),
                found: BTreeSet::new(),
            };
            w.value(root);
            for (tool, m) in w.found {
                out += &format!("K {k} {ty} {tool} {}\n", m as u8);
            }
        }
    }
    let unarmed = ctx.store(UNARMED)?;
    let mut keys: Vec<&String> = unarmed.own.keys().collect();
    keys.sort();
    for k in keys {
        let a = unarmed.resolved(k, "ItemToolSpec");
        let gt = a.str_at("GatherType").unwrap_or_else(|| k.clone());
        out += &format!(
            "U {gt} {} {} {}\n",
            a.f32_or("Power", 0.0),
            a.i32_or("Quality", 0),
            sound(ctx, a.str_at("HitSoundLayer"))?,
        );
    }
    if let Ok(gameplay) = ctx.store(GAMEPLAY) {
        let d = gameplay.resolved("Default", "GameplayConfig");
        if let Some(p) = d
            .obj("ItemDurability")
            .and_then(|x| x.obj("BrokenPenalties"))
        {
            out += &format!("X tool {}\n", p.f64_or("Tool", 0.0));
            out += &format!("X armor {}\n", p.f64_or("Armor", 0.0));
        }
        if let Some(g) = d.obj("Gathering") {
            for (name, key) in [
                ("unbreakable", "UnbreakableBlock"),
                ("incorrect", "IncorrectTool"),
            ] {
                if let Some(e) = g.obj(key) {
                    out += &format!(
                        "E {name} {} {}\n",
                        or_dash(e.str_at("ParticleSystemId")),
                        sound(ctx, e.str_at("SoundEventId"))?,
                    );
                }
            }
        }
    }
    Ok(out)
}
