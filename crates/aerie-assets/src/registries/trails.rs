//! `UpdateTrails` (48) from `Server/Entity/Trails`, keyed by name (spec 07).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | `TexturePath` | `texture` | `Trails/Basic.png` |
//! | `LifeSpan` | `life_span` | 0 |
//! | `Roll` | `roll` | 0 |
//! | `Start`, `End` `{Width (0), Color (color with alpha, white)}` | `start`, `end` | absent |
//! | `LightInfluence` | `light_influence` | 0 |
//! | `RenderMode` (enum name) | `render_mode` | `BlendLinear` |
//! | `IntersectionHighlight` `{HighlightThreshold, HighlightColor}` | same | absent |
//! | `Smooth` | `smooth` | `false` |
//! | `Animation.FrameSize` `{X, Y}` | `frame_size` | absent |
//! | `Animation.FrameRange` `{Min, Max}` (0 when missing) | `frame_range` | absent |
//! | `Animation.FrameLifeSpan` | `frame_life_span` | 0 |

use aerie_protocol::proto::{
    Edge, FXRenderMode, IntersectionHighlight, Range, Trail, UpdateTrails, Vector2i,
};
use serde_json::Value;

use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const PATH: &str = "Entity/Trails";

/// Edge color defaults to opaque white.
fn edge(v: &Value) -> Edge {
    let white = aerie_protocol::proto::ColorAlpha {
        alpha: -1,
        red: -1,
        green: -1,
        blue: -1,
    };
    Edge {
        color: Some(v.color_alpha("Color").unwrap_or(white)),
        width: v.f32_or("Width", 0.0),
    }
}

pub fn highlight(v: &Value) -> IntersectionHighlight {
    IntersectionHighlight {
        highlight_threshold: v.f32_or("HighlightThreshold", 0.0),
        highlight_color: v.color("HighlightColor"),
    }
}

pub fn range(v: &Value) -> Range {
    Range {
        min: v.i32_or("Min", 0),
        max: v.i32_or("Max", 0),
    }
}

fn trail(id: &str, a: &Value) -> Trail {
    let anim = a.obj("Animation");
    Trail {
        id: Some(id.to_owned()),
        texture: Some(
            a.str_at("TexturePath")
                .unwrap_or_else(|| "Trails/Basic.png".into()),
        ),
        life_span: a.i32_or("LifeSpan", 0),
        roll: a.f32_or("Roll", 0.0),
        start: a.obj("Start").map(edge),
        end: a.obj("End").map(edge),
        light_influence: a.f32_or("LightInfluence", 0.0),
        render_mode: enum_or(
            a,
            "RenderMode",
            FXRenderMode::from_name,
            FXRenderMode::BLEND_LINEAR,
        ),
        intersection_highlight: a.obj("IntersectionHighlight").map(highlight),
        smooth: a.bool_or("Smooth", false),
        frame_size: anim.and_then(|v| v.obj("FrameSize")).map(|s| Vector2i {
            x: s.i32_or("X", 0),
            y: s.i32_or("Y", 0),
        }),
        frame_range: anim.and_then(|v| v.obj("FrameRange")).map(range),
        frame_life_span: anim.map(|v| v.i32_or("FrameLifeSpan", 0)).unwrap_or(0),
    }
}

pub fn build(ctx: &Ctx) -> Result<UpdateTrails> {
    let store = ctx.store(PATH)?;
    let trails = store
        .assets
        .iter()
        .map(|(k, a)| (k.clone(), trail(k, a)))
        .collect();
    Ok(UpdateTrails {
        r#type: super::INIT,
        trails: Some(trails),
    })
}
