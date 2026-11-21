//! `UpdateConnectedBlockRuleSets` (90) from `Item/ConnectedBlockRuleSets`,
//! keyed by name (spec 07).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | key | `id` | |
//! | `UpdateMode` | `update_mode` | `PlaceUpdate` |
//! | `Shapes` `{id: shape}` (merged over the parent's) | `shapes` | empty |
//! | `Patterns` (the nearest in the chain) | `patterns` | absent |
//!
//! Shape (`Type`): `TagPattern` (`TagPattern`: key or inline), `Templated`
//! (`TemplateId`), `Block` (`Block` → block index); all with `FaceTags`
//! `{North, East, South, West, Up, Down: [tag]}` and `RelativeRotation`
//! `{Yaw, Pitch, Roll}` → rotation index (`roll·16 + pitch·4 + yaw`, each
//! 0-3 for `None`, `Ninety`, `OneEighty`, `TwoSeventy`).
//!
//! Pattern: `Rule`, `Output.ShapeId` (empty when absent),
//! `TransformRulesWithOrientation` (false), `RotationTransforms` (groups of
//! `{Yaw, Pitch, Roll}`, each `None`, `Ninety`, `OneEighty`, `TwoSeventy` or
//! `All`; every yaw × pitch × roll, first seen first) → `rotations` (`[0]`
//! when absent).
//!
//! Rule (`Type`): `FaceTag` (`PositionOffset`, `FaceTags`, `MatchType`
//! `All`, else `Any`), `Shape` (`PositionOffset`, `ShapeId`,
//! `AllowedRotations` group), `And`/`Or` (`Rules`), `Not` (`Rule`).

use aerie_protocol::proto as p;
use serde_json::Value;

use super::{blocks, tag_patterns};
use crate::chm::hashmap_order_from;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;
use crate::{Result, store::Store};

pub const PATH: &str = "Item/ConnectedBlockRuleSets";

fn rotation_ordinal(s: &str) -> Option<i32> {
    match s {
        "None" => Some(0),
        "Ninety" => Some(1),
        "OneEighty" => Some(2),
        "TwoSeventy" => Some(3),
        _ => None,
    }
}

fn rotation_index(v: Option<&Value>) -> i32 {
    let Some(v) = v else { return 0 };
    let r = |k: &str| {
        v.str_at(k)
            .as_deref()
            .and_then(rotation_ordinal)
            .unwrap_or(0)
    };
    r("Roll") * 16 + r("Pitch") * 4 + r("Yaw")
}

fn allowed(v: Option<String>) -> Vec<i32> {
    match v.as_deref() {
        Some("All") => vec![0, 1, 2, 3],
        Some(x) => vec![rotation_ordinal(x).unwrap_or(0)],
        None => vec![0],
    }
}

/// Rotation indexes of `{Yaw, Pitch, Roll}` groups, first seen first.
fn group_rotations(groups: &[&Value]) -> Vec<i32> {
    let mut out = Vec::new();
    for g in groups {
        for y in allowed(g.str_at("Yaw")) {
            for pi in allowed(g.str_at("Pitch")) {
                for r in allowed(g.str_at("Roll")) {
                    let i = r * 16 + pi * 4 + y;
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
        }
    }
    out
}

fn face_tags(v: Option<&Value>) -> Option<p::ConnectedBlockFaceTags> {
    let v = v?;
    let side = |k: &str| {
        let tags = v.strings(k)?;
        let mut uniq: Vec<String> = Vec::new();
        for t in &tags {
            if !uniq.contains(t) {
                uniq.push(t.clone());
            }
        }
        (!uniq.is_empty()).then(|| hashmap_order_from(&uniq, tags.len()))
    };
    Some(p::ConnectedBlockFaceTags {
        north: side("North"),
        east: side("East"),
        south: side("South"),
        west: side("West"),
        up: side("Up"),
        down: side("Down"),
    })
}

fn vec3i(v: Option<&Value>) -> p::Vector3i {
    match v {
        Some(v) => p::Vector3i {
            x: v.i32_or("X", 0),
            y: v.i32_or("Y", 0),
            z: v.i32_or("Z", 0),
        },
        None => p::Vector3i::default(),
    }
}

fn shape(ctx: &Ctx, v: &Value) -> Result<p::ConnectedBlockShape> {
    let tags = face_tags(v.get("FaceTags"));
    let rot = rotation_index(v.get("RelativeRotation"));
    Ok(match v.str_at("Type").as_deref() {
        Some("Templated") => p::ConnectedBlockShape::TemplatedConnectedBlockShape(Box::new(
            p::TemplatedConnectedBlockShape {
                face_tags: tags,
                relative_rotation_index: rot,
                template_id: v.str_at("TemplateId"),
            },
        )),
        Some("Block") => p::ConnectedBlockShape::BlockTypeConnectedBlockShape(Box::new(
            p::BlockTypeConnectedBlockShape {
                face_tags: tags,
                relative_rotation_index: rot,
                block_id: ctx.index_or(
                    blocks::PATH,
                    blocks::BUILTIN,
                    v.str_at("Block").as_deref(),
                    i32::MIN,
                )?,
            },
        )),
        _ => {
            let pattern = match v.get("TagPattern") {
                Some(Value::String(k)) => {
                    let s = ctx.store(tag_patterns::PATH)?;
                    s.own
                        .contains_key(k)
                        .then(|| tag_patterns::pattern(ctx, &s.resolved(k, "TagPattern")))
                }
                Some(o @ Value::Object(_)) => Some(tag_patterns::pattern(ctx, o)),
                _ => None,
            };
            p::ConnectedBlockShape::TagPatternConnectedBlockShape(Box::new(
                p::TagPatternConnectedBlockShape {
                    face_tags: tags,
                    relative_rotation_index: rot,
                    tag_pattern: pattern,
                },
            ))
        }
    })
}

fn rule(v: &Value) -> p::ConnectedBlockRule {
    let rules = |v: &Value| {
        v.get("Rules")
            .and_then(Value::as_array)
            .map(|l| l.iter().map(rule).collect())
    };
    match v.str_at("Type").as_deref() {
        Some("Shape") => {
            p::ConnectedBlockRule::ShapeConnectedBlockRule(Box::new(p::ShapeConnectedBlockRule {
                position_offset: vec3i(v.get("PositionOffset")),
                shape_id: v.str_at("ShapeId").unwrap_or_default(),
                allowed_rotations: v.get("AllowedRotations").map(|g| group_rotations(&[g])),
            }))
        }
        Some("And") => {
            p::ConnectedBlockRule::AndConnectedBlockRule(Box::new(p::AndConnectedBlockRule {
                rules: rules(v),
            }))
        }
        Some("Or") => {
            p::ConnectedBlockRule::OrConnectedBlockRule(Box::new(p::OrConnectedBlockRule {
                rules: rules(v),
            }))
        }
        Some("Not") => {
            p::ConnectedBlockRule::NotConnectedBlockRule(Box::new(p::NotConnectedBlockRule {
                rule: v.get("Rule").map(rule).unwrap_or_default(),
            }))
        }
        _ => p::ConnectedBlockRule::FaceTagConnectedBlockRule(Box::new(
            p::FaceTagConnectedBlockRule {
                position_offset: vec3i(v.get("PositionOffset")),
                face_tags: face_tags(v.get("FaceTags")),
                match_type: if v.str_at("MatchType").as_deref() == Some("All") {
                    p::ConnectedBlockMatchType::ALL
                } else {
                    p::ConnectedBlockMatchType::ANY
                },
            },
        )),
    }
}

fn pattern(v: &Value) -> p::ConnectedBlockPattern {
    let groups: Vec<&Value> = v
        .get("RotationTransforms")
        .and_then(Value::as_array)
        .map(|l| l.iter().collect())
        .unwrap_or_default();
    p::ConnectedBlockPattern {
        rule: v.get("Rule").map(rule),
        rotations: Some(if v.get("RotationTransforms").is_some() {
            group_rotations(&groups)
        } else {
            vec![0]
        }),
        transform_rules_with_orientation: v.bool_or("TransformRulesWithOrientation", false),
        output_shape_id: v
            .obj("Output")
            .and_then(|o| o.str_at("ShapeId"))
            .unwrap_or_default(),
    }
}

/// `Shapes` merged down the chain (parent first, own entries over it).
fn shapes(store: &Store, k: &str, depth: usize) -> Vec<(String, Value)> {
    let mut out = match store.parents.get(k).filter(|p| store.own.contains_key(*p)) {
        Some(p) if depth < 32 => shapes(store, p, depth + 1),
        _ => Vec::new(),
    };
    if let Some(m) = store
        .own
        .get(k)
        .and_then(|o| o.obj("Shapes"))
        .and_then(Value::as_object)
    {
        for (n, s) in m {
            out.retain(|(x, _)| x != n);
            out.push((n.clone(), s.clone()));
        }
    }
    out
}

pub fn build(ctx: &Ctx) -> Result<p::UpdateConnectedBlockRuleSets> {
    let store = ctx.store(PATH)?;
    let mut list = Vec::new();
    for k in store.own.keys() {
        let a = store.resolved(k, "PatternedConnectedBlockRuleSetAsset");
        let mut sh = Vec::new();
        for (n, s) in shapes(&store, k, 0) {
            sh.push((n, shape(ctx, &s)?));
        }
        list.push((
            k.clone(),
            p::ConnectedBlockRuleSetAsset {
                id: k.clone(),
                update_mode: enum_or(
                    &a,
                    "UpdateMode",
                    p::ConnectedBlockUpdateMode::from_name,
                    p::ConnectedBlockUpdateMode::PLACE_UPDATE,
                ),
                shapes: Some(sh),
                patterns: a
                    .get("Patterns")
                    .and_then(Value::as_array)
                    .map(|l| l.iter().map(pattern).collect()),
            },
        ));
    }
    Ok(p::UpdateConnectedBlockRuleSets {
        r#type: super::INIT,
        rule_sets: Some(list),
    })
}
