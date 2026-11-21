//! Items used up by holding an interaction (eating, drinking), for the
//! server: the official server runs the item's interaction chain and its
//! `ModifyInventory` takes `AdjustHeldItemQuantity` from the held stack.
//!
//! Each item's `Interactions` root is followed through `Serial`
//! (`Interactions`), `Condition` (`Next`), `Replace` (the item's
//! `InteractionVars[Var].Interactions`, else `DefaultValue.Interactions`),
//! `Charging` (`Next` by charge time) and references to `Item/Interactions`
//! (inline objects inherit their `Parent`). Where a `Charging` branch leads to
//! a `ModifyInventory`, the item is written as `F item type charge adjust
//! adventure`: interaction type number, the least charge time that reaches
//! it, the quantity change, and 1 when a `Condition` on the way requires
//! Adventure mode.

use serde_json::{Map, Value};

use super::root_interactions::{INTERACTIONS, PATH as ROOTS};
use crate::Result;
use crate::ctx::Ctx;
use crate::store::Store;
use crate::values::Get;
use aerie_protocol::proto::InteractionType;

struct Walk<'a> {
    roots: &'a Store,
    interactions: &'a Store,
    vars: Option<&'a Value>,
}

/// What a path reaches: (charge time, adjust, adventure only).
type Found = Option<(f32, i32, bool)>;

fn better(a: Found, b: Found) -> Found {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.0 < x.0 { y } else { x }),
        (x, None) => x,
        (None, y) => y,
    }
}

impl Walk<'_> {
    /// An interaction given by name or inline, made whole.
    fn node(&self, v: &Value) -> Option<Value> {
        match v {
            Value::String(name) => self
                .interactions
                .own
                .contains_key(name.as_str())
                .then(|| self.interactions.resolved(name, "Interaction")),
            Value::Object(o) => {
                let mut base = match o.get("Parent").and_then(Value::as_str) {
                    Some(p) if self.interactions.own.contains_key(p) => {
                        match self.interactions.resolved(p, "Interaction") {
                            Value::Object(b) => b,
                            _ => Map::new(),
                        }
                    }
                    _ => Map::new(),
                };
                for (k, x) in o {
                    if k != "Parent" {
                        base.insert(k.clone(), x.clone());
                    }
                }
                Some(Value::Object(base))
            }
            _ => None,
        }
    }

    fn list(&self, v: Option<&Value>, charge: Option<f32>, adv: bool, depth: u32) -> Found {
        let mut out = None;
        for x in v.and_then(Value::as_array).into_iter().flatten() {
            out = better(out, self.walk(x, charge, adv, depth));
        }
        out
    }

    /// `charge`: the charge time of the `Charging` branch we are in.
    fn walk(&self, v: &Value, charge: Option<f32>, adv: bool, depth: u32) -> Found {
        if depth > 24 {
            return None;
        }
        let n = self.node(v)?;
        let d = depth + 1;
        match n.str_at("Type").as_deref() {
            Some("ModifyInventory") => {
                let adjust = n.i32_or("AdjustHeldItemQuantity", 0);
                match charge {
                    Some(c) if adjust != 0 => Some((c, adjust, adv)),
                    _ => None,
                }
            }
            Some("Serial") | Some("Parallel") => self.list(n.get("Interactions"), charge, adv, d),
            Some("Condition") => {
                let adv = adv || n.str_at("RequiredGameMode").as_deref() == Some("Adventure");
                n.get("Next").and_then(|x| self.walk(x, charge, adv, d))
            }
            Some("Replace") => {
                let var = n.str_at("Var");
                let from_item = var
                    .as_deref()
                    .and_then(|k| self.vars?.get(k))
                    .and_then(|x| x.get("Interactions"));
                let list = from_item.or_else(|| n.get("DefaultValue")?.get("Interactions"));
                self.list(list, charge, adv, d)
            }
            Some("Charging") => {
                let mut out = None;
                for (k, x) in n
                    .get("Next")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    if let Ok(c) = k.parse::<f32>() {
                        out = better(out, self.walk(x, Some(c), adv, d));
                    }
                }
                out
            }
            _ => n.get("Next").and_then(|x| self.walk(x, charge, adv, d)),
        }
    }

    fn root(&self, v: &Value, depth: u32) -> Found {
        let r = match v {
            Value::String(name) if self.roots.own.contains_key(name.as_str()) => {
                self.roots.resolved(name, "RootInteraction")
            }
            Value::Object(_) => v.clone(),
            _ => return None,
        };
        self.list(r.get("Interactions"), None, false, depth)
    }
}

pub fn table(ctx: &Ctx) -> Result<String> {
    let roots = ctx.store(ROOTS)?;
    let interactions = ctx.store(INTERACTIONS)?;
    let items = ctx.store(super::items::PATH)?;
    let mut out = String::new();
    let mut keys: Vec<&String> = items.own.keys().collect();
    keys.sort();
    for k in keys {
        let a = items.resolved(k, "Item");
        let Some(map) = a.obj("Interactions").and_then(Value::as_object) else {
            continue;
        };
        let w = Walk {
            roots: &roots,
            interactions: &interactions,
            vars: a.obj("InteractionVars"),
        };
        for (t, root) in map {
            let Some(ty) = InteractionType::from_name(t) else {
                continue;
            };
            if let Some((charge, adjust, adv)) = w.root(root, 0) {
                out += &format!("F {k} {} {charge} {adjust} {}\n", ty.0, adv as u8);
            }
        }
    }
    Ok(out)
}
