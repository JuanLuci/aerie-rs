//! What block physics needs beyond the client's block registry (spec 10):
//! per block with any of them, `SupportDropType`, the `Gathering.Physics`
//! drop and `Gathering.UseDefaultDropWhenPlaced`; and every drop list.
//!
//! Falling blocks (spec 24) add `F key impact dropItems collision`.
//! Connected block templates (spec 23) add `CT template <json>` (the
//! template's shapes in the order the official server walks them),
//! `CB key owner template <json>` (a block's shape → blocks map; `owner`
//! is the block that declares it) and `CF key flip` (1 orthogonal,
//! 2 orthogonal inverse).
//!
//! Text lines for the start-up cache: `P key drop item list deco`
//! (drop: 0 Break, 1 Destroy, 2 Fall; `-` for none; deco 1 when placed
//! blocks become decoration) and `L list <json>` (the list's `Container`,
//! one line of JSON).

use serde_json::Value;

use super::music::{owner_of, reference};
use crate::Result;
use crate::ctx::Ctx;
use crate::values::Get;

pub const DROPS: &str = "Drops";

fn or_dash(s: Option<String>) -> String {
    s.filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
        .unwrap_or_else(|| "-".into())
}

/// The connected block templates.
pub const TEMPLATES: &str = "Item/CustomConnectedBlockTemplates";

/// Quarter turns of a rotation named in degrees.
fn quarter(v: Option<&Value>) -> i64 {
    match v.and_then(Value::as_str) {
        Some("Ninety") => 1,
        Some("OneEighty") => 2,
        Some("TwoSeventy") => 3,
        _ => 0,
    }
}

/// `Name|Yaw=90|Filler=x,y,z` → (name, rotation index, packed filler).
fn entry(text: &str) -> (String, i64, i64) {
    let mut parts = text.split('|');
    let name = parts.next().unwrap_or("").to_owned();
    let (mut rot, mut filler) = (0, 0);
    for p in parts {
        let Some((k, v)) = p.split_once('=') else {
            continue;
        };
        let turns = |v: &str| v.parse::<i64>().map_or(0, |d| d.rem_euclid(360) / 90);
        match k {
            "Yaw" => rot += turns(v),
            "Pitch" => rot += turns(v) * 4,
            "Roll" => rot += turns(v) * 16,
            "Filler" => {
                let n: Vec<i64> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                if let [x, y, z] = n[..] {
                    filler = (x & 31) | (z & 31) << 5 | (y & 31) << 10;
                }
            }
            _ => {}
        }
    }
    (name, rot, filler)
}

/// A block pattern (`A,50%B|Yaw=90`) as its entries: (block, rotation).
fn pattern_entries(text: &str) -> Vec<Value> {
    // A filler's commas are not separators.
    let mut fixed = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("Filler=") {
        let (head, tail) = rest.split_at(i);
        fixed += head;
        let end = tail.find(['|', ';']).unwrap_or(tail.len());
        let commas = tail[..end].matches(',').count();
        let cut = if commas > 2 {
            tail.match_indices(',').nth(2).map_or(end, |m| m.0)
        } else {
            end
        };
        fixed += &tail[..cut].replace(',', "\u{1}");
        rest = &tail[cut..];
    }
    fixed += rest;
    fixed
        .replace(';', ",")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let s = s.replace('\u{1}', ",");
            let s = s.split_once('%').map_or(s.as_str(), |x| x.1);
            let (name, rot, _) = entry(s);
            serde_json::json!([name, rot])
        })
        .collect()
}

fn face_tags(v: Option<&Value>) -> Value {
    let mut out = serde_json::Map::new();
    for (name, dir) in [
        ("North", [0, 0, -1]),
        ("East", [1, 0, 0]),
        ("South", [0, 0, 1]),
        ("West", [-1, 0, 0]),
        ("Up", [0, 1, 0]),
        ("Down", [0, -1, 0]),
    ] {
        if let Some(tags) = v.and_then(|v| v.get(name)).and_then(Value::as_array) {
            out.insert(
                format!("{},{},{}", dir[0], dir[1], dir[2]),
                Value::Array(tags.clone()),
            );
        }
    }
    Value::Object(out)
}

fn template(a: &Value) -> Value {
    let shapes = a.get("Shapes").and_then(Value::as_object);
    let names: Vec<String> = shapes
        .map(|s| s.keys().cloned().collect())
        .unwrap_or_default();
    let ordered: Vec<Value> = crate::chm::hashmap_order(&names)
        .iter()
        .map(|name| {
            let s = &shapes.unwrap()[name];
            let patterns: Vec<Value> = s
                .get("PatternsToMatchAnyOf")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|p| {
                    let t = p.get("AllowedPatternTransformations");
                    let flag = |k: &str| t.is_some_and(|t| t.bool_or(k, false));
                    let rules: Vec<Value> = p
                        .get("RulesToMatch")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|r| {
                            let at = |k: &str| {
                                r.get("Position")
                                    .and_then(|p| p.get(k))
                                    .and_then(Value::as_i64)
                                    .unwrap_or(0)
                            };
                            let normals = r.get("PlacementNormals").and_then(Value::as_array).map(
                                |n| {
                                    n.iter()
                                        .filter_map(|s| match s.as_str()? {
                                            "Up" => Some([0, 1, 0]),
                                            "Down" => Some([0, -1, 0]),
                                            "North" => Some([0, 0, -1]),
                                            "East" => Some([1, 0, 0]),
                                            "South" => Some([0, 0, 1]),
                                            "West" => Some([-1, 0, 0]),
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>()
                                },
                            );
                            let shapes: Vec<Value> = r
                                .get("Shapes")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(|s| {
                                    let (n, rot, filler) = entry(s);
                                    serde_json::json!([n, rot, filler])
                                })
                                .collect();
                            serde_json::json!({
                                "pos": [at("X"), at("Y"), at("Z")],
                                "include": r.str_at("IncludeOrExclude").as_deref() == Some("Include"),
                                "normals": normals,
                                "tags": face_tags(r.get("FaceTags")),
                                "shapes": shapes,
                                "blocks": r.get("BlockTypes").cloned().unwrap_or(Value::Array(vec![])),
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "orient": p.bool_or("TransformRulesToOrientation", true),
                        "yaw": quarter(p.get("YawToApplyAddReplacedBlockType")),
                        "turns": flag("IsCardinallyRotatable"),
                        "mirror_x": flag("MirrorX"),
                        "mirror_z": flag("MirrorZ"),
                        "placement": p.bool_or("OnlyOnPlacement", false),
                        "update": p.bool_or("OnlyOnUpdate", false),
                        "rules": rules,
                    })
                })
                .collect();
            serde_json::json!({
                "name": name,
                "tags": face_tags(s.get("FaceTags")),
                "patterns": patterns,
            })
        })
        .collect();
    serde_json::json!({
        "default": a.str_at("DefaultShape"),
        "others": a.bool_or("ConnectsToOtherMaterials", true),
        "once": a.bool_or("DontUpdateAfterInitialPlacement", false),
        "shapes": ordered,
    })
}

pub fn table(ctx: &Ctx) -> Result<String> {
    let mut out = String::new();
    if let Ok(templates) = ctx.store(TEMPLATES) {
        let mut keys: Vec<&String> = templates.own.keys().collect();
        keys.sort();
        for k in keys {
            if k.contains(char::is_whitespace) {
                continue;
            }
            let a = templates.resolved(k, "CustomConnectedBlockTemplateAsset");
            out += &format!("CT {k} {}\n", template(&a));
        }
    }
    let blocks = ctx.store(super::blocks::PATH)?;
    let mut keys: Vec<&String> = blocks.own.keys().collect();
    keys.sort();
    for k in keys {
        let a = blocks.resolved(k, "BlockType");
        // A door, and whether it is a horizontal one (its use
        // interaction says so): `D key horizontal`.
        if a.bool_or("IsDoor", false) {
            let horizontal = a
                .obj("Interactions")
                .and_then(|i| i.str_at("Use"))
                .is_some_and(|u| u.contains("Horizontal"));
            out += &format!("D {k} {}\n", horizontal as u8);
        }
        // The sound of each interaction state: `S key state sound`.
        for (state, def) in a
            .obj("State")
            .and_then(|s| s.obj("Definitions"))
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if state.contains(char::is_whitespace) {
                continue;
            }
            let sound =
                super::sound_events::index(ctx, def.str_at("InteractionSoundEventId").as_deref())?;
            if sound > 0 {
                out += &format!("S {k} {state} {sound}\n");
            }
        }
        // A block that holds items: `K key capacity stayOpenWhenEmpty
        // openSound closeSound` (the sounds of its window states).
        if let Some(c) = a
            .obj("BlockEntity")
            .and_then(|e| e.obj("Components"))
            .and_then(|c| c.obj("ItemContainerBlock"))
        {
            let sound = |state: &str| -> Result<i32> {
                let id = a
                    .obj("State")
                    .and_then(|s| s.obj("Definitions"))
                    .and_then(|d| d.obj(state))
                    .and_then(|s| s.str_at("InteractionSoundEventId"));
                super::sound_events::index(ctx, id.as_deref())
            };
            out += &format!(
                "K {k} {} {} {} {}\n",
                c.get("Capacity").and_then(Value::as_i64).unwrap_or(20),
                c.bool_or("StayOpenWhenEmpty", false) as u8,
                sound("OpenWindow")?,
                sound("CloseWindow")?
            );
        }
        // A block that fits itself to its neighbours by a template.
        if let Some(r) = a.obj("ConnectedBlockRuleSet")
            && r.str_at("Type").as_deref() == Some("CustomTemplate")
            && let Some(t) = r.str_at("TemplateShapeAssetId")
            && !k.contains(char::is_whitespace)
        {
            let owner = owner_of(&blocks, k, "ConnectedBlockRuleSet");
            let map: serde_json::Map<String, Value> = r
                .get("TemplateShapeBlockPatterns")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter_map(|(shape, v)| {
                    Some((shape.clone(), Value::Array(pattern_entries(v.as_str()?))))
                })
                .collect();
            out += &format!("CB {k} {owner} {t} {}\n", Value::Object(map));
        }
        match a.str_at("FlipType").as_deref() {
            Some("ORTHOGONAL") | Some("Orthogonal") => out += &format!("CF {k} 1\n"),
            Some("ORTHOGONAL_INVERSE") | Some("OrthogonalInverse") => out += &format!("CF {k} 2\n"),
            _ => {}
        }
        // What a falling block does when it lands: `F key impact
        // dropItems collision` (impact: 0 Place, 1 Break, 2 Explode;
        // collision: the hitbox collision config's index, -1 for none).
        if let Some(f) = a.obj("FallingBlockSettings") {
            let impact = f.obj("Impact");
            let kind = match impact.and_then(|i| i.str_at("Type")).as_deref() {
                Some("Break") => 1,
                Some("Explode") => 2,
                _ => 0,
            };
            let drops = impact.is_none_or(|i| i.bool_or("DropItems", true));
            let collision = ctx.index_or(
                super::entity_configs::HITBOX,
                &[],
                f.str_at("HitboxCollisionConfig").as_deref(),
                -1,
            )?;
            out += &format!("F {k} {kind} {} {collision}\n", drops as u8);
        }
        let drop = match a.str_at("SupportDropType").as_deref() {
            Some("Destroy") => 1,
            Some("Fall") => 2,
            _ => 0,
        };
        let g = a.obj("Gathering");
        let physics = g.and_then(|g| g.obj("Physics"));
        let deco = a.bool_or("IgnoreSupportWhenPlaced", false)
            || g.is_some_and(|g| g.bool_or("UseDefaultDropWhenPlaced", false));
        if drop == 0 && physics.is_none() && !deco {
            continue;
        }
        let owner = owner_of(&blocks, k, "Gathering");
        let (item, list) = match physics {
            Some(p) => (
                p.str_at("ItemId"),
                p.get("DropList")
                    .and_then(|d| reference(d, &owner, "Gathering_Physics_DropList")),
            ),
            None => (None, None),
        };
        // `-` in the item column with a physics entry means "no item".
        let item = match (physics.is_some(), item) {
            (true, None) => "!".to_owned(),
            (_, i) => or_dash(i),
        };
        out += &format!("P {k} {drop} {item} {} {}\n", or_dash(list), deco as u8);
    }
    let lists = ctx.store(DROPS)?;
    let mut keys: Vec<&String> = lists.own.keys().collect();
    keys.sort();
    for k in keys {
        if k.contains(char::is_whitespace) {
            continue;
        }
        let a = lists.resolved(k, "ItemDropList");
        let c = a.get("Container").cloned().unwrap_or(Value::Null);
        out += &format!("L {k} {}\n", c);
    }
    Ok(out)
}
