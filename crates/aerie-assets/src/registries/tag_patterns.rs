//! `UpdateTagPatterns` (84) from `Server/TagPatterns` (indexed, no built-in
//! entries) (spec 07).
//!
//! A pattern is `{"Op": "Equals", "Tag": name}`, `{"Op": "And" | "Or",
//! "Patterns": [...]}` or `{"Op": "Not", "Pattern": {...}}`. `Equals` sends the
//! tag's index; `And`/`Or` send their operands; `Not` sends its pattern.

use aerie_protocol::proto::{TagPattern, TagPatternType, UpdateTagPatterns};
use aerie_protocol::wire::Wire;
use serde_json::Value;

use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub const PATH: &str = "TagPatterns";

pub fn pattern(ctx: &Ctx, v: &Value) -> TagPattern {
    match v.str_at("Op").as_deref() {
        Some("And") | Some("Or") => TagPattern {
            r#type: if v.str_at("Op").as_deref() == Some("And") {
                TagPatternType::AND
            } else {
                TagPatternType::OR
            },
            operands: Some(
                v.get("Patterns")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|p| pattern(ctx, p)).collect())
                    .unwrap_or_default(),
            ),
            ..Default::default()
        },
        Some("Not") => TagPattern {
            r#type: TagPatternType::NOT,
            not: v.get("Pattern").map(|p| Box::new(pattern(ctx, p))),
            ..Default::default()
        },
        _ => TagPattern {
            r#type: TagPatternType::EQUALS,
            tag_index: v.str_at("Tag").map(|t| ctx.tag(&t)).unwrap_or(0),
            ..Default::default()
        },
    }
}

pub fn build(ctx: &Ctx) -> Result<UpdateTagPatterns> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, &[])?;
    let map = order
        .iter()
        .enumerate()
        .map(|(i, k)| (i as i32, pattern(ctx, &store.assets[k])))
        .collect();
    Ok(UpdateTagPatterns {
        r#type: super::INIT,
        max_id: order.len() as i32,
        patterns: Some(map),
    })
}

/// Learns tag indexes from a captured `UpdateTagPatterns`: patterns have no
/// parents, so their order is stable and pattern i of the capture is our
/// pattern i; each `Equals` then pairs a tag name with the official index.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    fn walk(ctx: &Ctx, v: &Value, p: &TagPattern) {
        match v.str_at("Op").as_deref() {
            Some("And") | Some("Or") => {
                let ops = v
                    .get("Patterns")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for (jv, jp) in ops.iter().zip(p.operands.iter().flatten()) {
                    walk(ctx, jv, jp);
                }
            }
            Some("Not") => {
                if let (Some(jv), Some(jp)) = (v.get("Pattern"), &p.not) {
                    walk(ctx, jv, jp);
                }
            }
            _ => {
                if let Some(t) = v.str_at("Tag") {
                    ctx.pin_tag(&t, p.tag_index);
                }
            }
        }
    }
    let (cap, _) = <UpdateTagPatterns as Wire>::decode(captured, 0)?;
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, &[])?;
    for (i, p) in cap.patterns.unwrap_or_default() {
        if let Some(k) = order.get(i as usize) {
            walk(ctx, &store.assets[k], &p);
        }
    }
    Ok(())
}
