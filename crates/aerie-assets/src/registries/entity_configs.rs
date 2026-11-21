//! Small entity registries, all indexed without built-in entries (spec 07).
//! Inheritance follows the schema (`Store::resolved`).
//!
//! - `UpdateHitboxCollisionConfig` (74) from `Entity/HitboxCollision`:
//!   `CollisionType` (enum, `Hard`), `SoftCollisionOffsetRatio` (1),
//!   `RotateHitbox` (false), `AllowEntityAnchoring` (true),
//!   `RotateAnchoredEntities` (false).
//! - `UpdateRepulsionConfig` (75) from `Entity/Repulsion`: `Radius`,
//!   `MinForce`, `MaxForce` (0).
//! - `UpdateBeamConfigs` (93) from `Entity/Beams`: `TexturePath` → `texture`.
//! - `UpdateEmotes` (86) from `Emote`: `Name`, `Animation`, `Icon`,
//!   `IsLooping`, `HideItemInHand`; the key is `id`. (The folder is empty in
//!   the current game.)
//! - `UpdateEntityUIComponents` (73) from `Entity/UI`, by `Type`:
//!   every component has `HitboxOffset` `{X, Y}` (0, 0). `EntityStat`:
//!   `EntityStat` → `entity_stat_index` (0 when unknown). `CombatText`:
//!   `RandomPositionOffsetRange` `{X: {Min, Max}, Y: …}`, `ViewportMargin`,
//!   `Duration` (0), `HitAngleModifierStrength` (1), `FontSize` (68),
//!   `TextColor` (white), `AnimationEvents` (list by `Type`: `Scale` with
//!   `StartScale`/`EndScale`, `Position` with `PositionOffset` `{X, Y}`,
//!   `Opacity` with `StartOpacity`/`EndOpacity`; all with `StartAt`/`EndAt`).

use aerie_protocol::proto::{
    BeamConfig, CollisionType, Color, CombatTextEntityUIAnimationEventType,
    CombatTextEntityUIComponentAnimationEvent, EntityUIComponent, EntityUIType,
    HitboxCollisionConfig, ProtocolEmote, RangeVector2f, RepulsionConfig, UpdateBeamConfigs,
    UpdateEmotes, UpdateEntityUIComponents, UpdateHitboxCollisionConfig, UpdateRepulsionConfig,
};
use serde_json::Value;

use super::{entity_stat_types, particles::rangef};
use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const HITBOX: &str = "Entity/HitboxCollision";
pub const REPULSION: &str = "Entity/Repulsion";
pub const BEAMS: &str = "Entity/Beams";
pub const EMOTES: &str = "Emote";
pub const UI: &str = "Entity/UI";

/// Each asset of an indexed folder, resolved, with its index.
fn indexed<T>(
    ctx: &Ctx,
    path: &str,
    class: &str,
    mut f: impl FnMut(&str, &Value) -> Result<T>,
) -> Result<(i32, Vec<(i32, T)>)> {
    let store = ctx.store(path)?;
    let order = ctx.order(path, &[])?;
    let idx = ctx.indexes(path, &[])?;
    let mut out = Vec::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        out.push((i, f(k, &store.resolved(k, class))?));
    }
    Ok((order.len() as i32, out))
}

fn vec2(v: Option<&Value>) -> [f32; 2] {
    v.map(|v| [v.f32_or("X", 0.0), v.f32_or("Y", 0.0)])
        .unwrap_or([0.0, 0.0])
}

pub fn build_hitbox(ctx: &Ctx) -> Result<UpdateHitboxCollisionConfig> {
    let (max_id, list) = indexed(ctx, HITBOX, "HitboxCollisionConfig", |_, a| {
        Ok(HitboxCollisionConfig {
            collision_type: enum_or(
                a,
                "CollisionType",
                CollisionType::from_name,
                CollisionType::HARD,
            ),
            soft_collision_offset_ratio: a.f32_or("SoftCollisionOffsetRatio", 1.0),
            rotate_hitbox: a.bool_or("RotateHitbox", false),
            allow_entity_anchoring: a.bool_or("AllowEntityAnchoring", true),
            rotate_anchored_entities: a.bool_or("RotateAnchoredEntities", false),
        })
    })?;
    Ok(UpdateHitboxCollisionConfig {
        r#type: super::INIT,
        max_id,
        hitbox_collision_configs: Some(list),
    })
}

pub fn build_repulsion(ctx: &Ctx) -> Result<UpdateRepulsionConfig> {
    let (max_id, list) = indexed(ctx, REPULSION, "RepulsionConfig", |_, a| {
        Ok(RepulsionConfig {
            radius: a.f32_or("Radius", 0.0),
            min_force: a.f32_or("MinForce", 0.0),
            max_force: a.f32_or("MaxForce", 0.0),
        })
    })?;
    Ok(UpdateRepulsionConfig {
        r#type: super::INIT,
        max_id,
        repulsion_configs: Some(list),
    })
}

pub fn build_beams(ctx: &Ctx) -> Result<UpdateBeamConfigs> {
    let (max_id, list) = indexed(ctx, BEAMS, "Beam", |_, a| {
        Ok(BeamConfig {
            texture: a.str_at("TexturePath"),
        })
    })?;
    Ok(UpdateBeamConfigs {
        r#type: super::INIT,
        max_id,
        beam_configs: Some(list),
    })
}

pub fn build_emotes(ctx: &Ctx) -> Result<UpdateEmotes> {
    let (max_id, list) = indexed(ctx, EMOTES, "EmoteAsset", |k, a| {
        Ok(ProtocolEmote {
            id: Some(k.to_owned()),
            name: a.str_at("Name"),
            animation: a.str_at("Animation"),
            icon: a.str_at("Icon"),
            is_looping: a.bool_or("IsLooping", false),
            hide_item_in_hand: a.bool_or("HideItemInHand", false),
        })
    })?;
    Ok(UpdateEmotes {
        r#type: super::INIT,
        max_id,
        emotes: Some(list),
    })
}

fn animation_event(v: &Value) -> CombatTextEntityUIComponentAnimationEvent {
    let mut e = CombatTextEntityUIComponentAnimationEvent {
        start_at: v.f32_or("StartAt", 0.0),
        end_at: v.f32_or("EndAt", 0.0),
        ..Default::default()
    };
    match v.str_at("Type").as_deref() {
        Some("Scale") => {
            e.r#type = CombatTextEntityUIAnimationEventType::SCALE;
            e.start_scale = v.f32_or("StartScale", 0.0);
            e.end_scale = v.f32_or("EndScale", 0.0);
        }
        Some("Position") => {
            e.r#type = CombatTextEntityUIAnimationEventType::POSITION;
            e.position_offset = vec2(v.get("PositionOffset"));
        }
        Some("Opacity") => {
            e.r#type = CombatTextEntityUIAnimationEventType::OPACITY;
            e.start_opacity = v.f32_or("StartOpacity", 0.0);
            e.end_opacity = v.f32_or("EndOpacity", 0.0);
        }
        _ => {}
    }
    e
}

pub fn build_ui(ctx: &Ctx) -> Result<UpdateEntityUIComponents> {
    let (max_id, list) = indexed(ctx, UI, "EntityUIComponent", |_, a| {
        let mut c = EntityUIComponent {
            hitbox_offset: vec2(a.get("HitboxOffset")),
            ..Default::default()
        };
        match a.str_at("Type").as_deref() {
            Some("EntityStat") => {
                c.r#type = EntityUIType::ENTITY_STAT;
                c.entity_stat_index = ctx.index_or(
                    entity_stat_types::PATH,
                    entity_stat_types::BUILTIN,
                    a.str_at("EntityStat").as_deref(),
                    0,
                )?;
            }
            Some("CombatText") => {
                c.r#type = EntityUIType::COMBAT_TEXT;
                c.combat_text_random_position_offset_range = a
                    .obj("RandomPositionOffsetRange")
                    .map(|r| RangeVector2f {
                        x: r.obj("X").map(rangef),
                        y: r.obj("Y").map(rangef),
                    })
                    .unwrap_or_default();
                c.combat_text_viewport_margin = a.f32_or("ViewportMargin", 0.0);
                c.combat_text_duration = a.f32_or("Duration", 0.0);
                c.combat_text_hit_angle_modifier_strength =
                    a.f32_or("HitAngleModifierStrength", 1.0);
                c.combat_text_font_size = a.f32_or("FontSize", 68.0);
                c.combat_text_color = a.color("TextColor").unwrap_or(Color {
                    red: -1,
                    green: -1,
                    blue: -1,
                });
                c.combat_text_animation_events = a
                    .get("AnimationEvents")
                    .and_then(Value::as_array)
                    .map(|l| l.iter().map(animation_event).collect())
                    .unwrap_or_default();
            }
            _ => c.unknown = true,
        }
        Ok(c)
    })?;
    Ok(UpdateEntityUIComponents {
        r#type: super::INIT,
        max_id,
        components: Some(list),
    })
}
