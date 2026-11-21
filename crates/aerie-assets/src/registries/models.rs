//! Model pieces shared by several assets (spec 07).
//!
//! Animation set (a map entry; its key becomes `id`): `Animations` (list),
//! `NextAnimationDelay` `{Min, Max}` (2 to 10).
//!
//! Animation: `Animation` → `name`, `Speed` (1), `BlendingDuration` (0.2),
//! `Looping` (true), `Weight` (1), `FootstepIntervals` (empty),
//! `SoundEventId` → `sound_event_index` (0), `PassiveLoopCount` (1).

use aerie_protocol::proto::{
    Animation, AnimationSet, DetailBox, EntityPart, Hitbox, Model, ModelAttachment, ModelTrail,
    Phobia, Rangef,
};
use serde_json::Value;

use super::{particles::rangef, sound_events};
use crate::ctx::enum_or;
use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub fn animation(ctx: &Ctx, v: &Value) -> Result<Animation> {
    Ok(Animation {
        name: v.str_at("Animation"),
        speed: v.f32_or("Speed", 1.0),
        blending_duration: v.f32_or("BlendingDuration", 0.2),
        looping: v.bool_or("Looping", true),
        weight: v.f32_or("Weight", 1.0),
        footstep_intervals: v
            .get("FootstepIntervals")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_i64())
                    .map(|x| x as i32)
                    .collect()
            })
            .unwrap_or_default(),
        sound_event_index: sound_events::index(ctx, v.str_at("SoundEventId").as_deref())?,
        passive_loop_count: v.i32_or("PassiveLoopCount", 1),
    })
}

pub fn animation_set(ctx: &Ctx, id: &str, v: &Value) -> Result<AnimationSet> {
    let animations = match v.get("Animations").and_then(Value::as_array) {
        Some(a) => Some(
            a.iter()
                .map(|x| animation(ctx, x))
                .collect::<Result<Vec<_>>>()?,
        ),
        None => None,
    };
    Ok(AnimationSet {
        id: Some(id.to_owned()),
        animations,
        next_animation_delay: Some(v.obj("NextAnimationDelay").map(rangef).unwrap_or(Rangef {
            min: 2.0,
            max: 10.0,
        })),
    })
}

pub const MODELS: &str = "Models";

fn vec3d(v: Option<&Value>) -> [f32; 3] {
    match v {
        Some(v) => [v.f32_or("X", 0.0), v.f32_or("Y", 0.0), v.f32_or("Z", 0.0)],
        None => [0.0; 3],
    }
}

fn hitbox(b: &Value) -> Hitbox {
    let [min_x, min_y, min_z] = vec3d(b.get("Min"));
    let [max_x, max_y, max_z] = vec3d(b.get("Max"));
    Hitbox {
        min_x,
        min_y,
        min_z,
        max_x,
        max_y,
        max_z,
    }
}

fn attachment(a: &Value) -> ModelAttachment {
    ModelAttachment {
        model: a.str_at("Model"),
        texture: a.str_at("Texture"),
        gradient_set: a.str_at("GradientSet"),
        gradient_id: a.str_at("GradientId"),
    }
}

/// A model asset (`Models`) at unit scale, with its default attachments, as
/// `Model.toPacket` sends it (spec 07, "Models"). `None` when the asset does
/// not exist. `hitbox` overrides the asset's own `HitBox` (phobia models use
/// their owner's).
///
/// | JSON | Packet | Default |
/// | --- | --- | --- |
/// | key | `asset_id` | |
/// | `Model` | `path` | absent |
/// | `Texture` | `texture` | the model path with `.blockymodel` → `.png`; `textures/Unknown.png` without a model |
/// | `GradientSet`, `GradientId` | same | absent |
/// | `EyeHeight`, `CrouchOffset`, `SittingOffset`, `SleepingOffset` | same | 0 |
/// | `AnimationSets` | `animation_sets` | empty |
/// | `DefaultAttachments` | `attachments` | absent when empty |
/// | `HitBox` `{Min, Max}` | `hitbox` | absent |
/// | `Light`, `Particles`, `Trails`, `Camera` | same | absent |
/// | `DetailBoxes` `{name: [{Offset, Box}]}` | `detail_boxes` | empty |
/// | `Phobia`, `PhobiaModelAssetId` | `phobia`, `phobia_model` | `None` |
pub fn model(ctx: &Ctx, key: &str, hitbox_override: Option<&Value>) -> Result<Option<Model>> {
    let store = ctx.store(MODELS)?;
    let Some(k) = store
        .own
        .keys()
        .find(|k| k.eq_ignore_ascii_case(key))
        .cloned()
    else {
        return Ok(None);
    };
    let a = store.resolved(&k, "ModelAsset");
    let path = a.str_at("Model");
    let texture = a.str_at("Texture").or_else(|| match &path {
        Some(p) => Some(p.replace(".blockymodel", ".png")),
        None => Some("textures/Unknown.png".into()),
    });
    let hb = hitbox_override
        .or(a.get("HitBox"))
        .filter(|h| h.is_object());
    let mut animation_sets = Vec::new();
    for (name, s) in a
        .obj("AnimationSets")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        animation_sets.push((name.clone(), animation_set(ctx, name, s)?));
    }
    let detail_boxes = a
        .obj("DetailBoxes")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(n, l)| {
                    (
                        n.clone(),
                        l.as_array()
                            .into_iter()
                            .flatten()
                            .map(|d| DetailBox {
                                offset: vec3d(d.get("Offset")),
                                r#box: d.get("Box").map(hitbox),
                            })
                            .collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let phobia = enum_or(&a, "Phobia", Phobia::from_name, Phobia::default());
    let phobia_model = match a.str_at("PhobiaModelAssetId") {
        Some(p) if phobia != Phobia::default() => model(ctx, &p, hb)?.map(Box::new),
        _ => None,
    };
    let f = |key: &str| a.f32_or(key, 0.0);
    Ok(Some(Model {
        asset_id: Some(k.clone()),
        path,
        texture,
        gradient_set: a.str_at("GradientSet"),
        gradient_id: a.str_at("GradientId"),
        camera: a.obj("Camera").map(super::item_animations::camera),
        scale: 1.0,
        eye_height: f("EyeHeight").max(0.0),
        crouch_offset: f("CrouchOffset"),
        sitting_offset: f("SittingOffset"),
        sleeping_offset: f("SleepingOffset"),
        animation_sets: Some(animation_sets),
        attachments: a
            .get("DefaultAttachments")
            .and_then(Value::as_array)
            .filter(|l| !l.is_empty())
            .map(|l| l.iter().map(attachment).collect()),
        hitbox: hb.map(hitbox),
        particles: a
            .get("Particles")
            .and_then(Value::as_array)
            .filter(|l| !l.is_empty())
            .map(|l| l.iter().map(super::model_particles::one).collect()),
        trails: a.get("Trails").and_then(Value::as_array).map(|l| {
            let owner = super::music::owner_of(&store, &k, "Trails");
            l.iter()
                .enumerate()
                .map(|(i, t)| ModelTrail {
                    trail_id: t.get("TrailId").and_then(|x| {
                        super::music::reference(x, &owner, &format!("Trails_{i}_TrailId"))
                    }),
                    target_entity_part: enum_or(
                        t,
                        "TargetEntityPart",
                        EntityPart::from_name,
                        EntityPart::SELF,
                    ),
                    target_node_name: t.str_at("TargetNodeName"),
                    position_offset: t.obj("PositionOffset").map(|v| vec3d(Some(v))),
                    rotation_offset: t.obj("RotationOffset").map(crate::values::direction),
                    fixed_rotation: t.bool_or("FixedRotation", false),
                })
                .collect()
        }),
        light: a.obj("Light").map(super::fluids::light),
        detail_boxes: Some(detail_boxes),
        phobia: if phobia_model.is_some() {
            phobia
        } else {
            Phobia::default()
        },
        phobia_model,
    }))
}
