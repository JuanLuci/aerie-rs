//! Model particles, embedded in many assets (spec 07).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | `SystemId` | `system_id` | absent |
//! | `TargetEntityPart` (enum name) | `target_entity_part` | `Self` |
//! | `TargetNodeName` | `target_node_name` | absent |
//! | `Color` (color) | `color` | absent |
//! | `Scale` | `scale` | 1 |
//! | `PositionOffset` `{X,Y,Z}` | `position_offset` | absent |
//! | `RotationOffset` `{Yaw,Pitch,Roll}` | `rotation_offset` | absent |
//! | `DetachedFromModel`, `ClearParticlesOnRemove` | same | `false` |

use aerie_protocol::proto::{EntityPart, ModelParticle};
use serde_json::Value;

use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::{Get, direction, vec3};

pub fn one(v: &Value) -> ModelParticle {
    ModelParticle {
        system_id: v.str_at("SystemId"),
        scale: v.f32_or("Scale", 1.0),
        color: v.color("Color"),
        target_entity_part: enum_or(
            v,
            "TargetEntityPart",
            EntityPart::from_name,
            EntityPart::SELF,
        ),
        target_node_name: v.str_at("TargetNodeName"),
        position_offset: v.obj("PositionOffset").map(vec3),
        rotation_offset: v.obj("RotationOffset").map(direction),
        detached_from_model: v.bool_or("DetachedFromModel", false),
        clear_particles_on_remove: v.bool_or("ClearParticlesOnRemove", false),
    }
}

/// A JSON array of particles; empty when absent.
pub fn list(_ctx: &Ctx, v: Option<&Value>) -> Result<Vec<ModelParticle>> {
    Ok(v.and_then(Value::as_array)
        .map(|a| a.iter().map(one).collect())
        .unwrap_or_default())
}
