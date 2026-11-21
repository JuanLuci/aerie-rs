//! `UpdateRootInteractions` (67) from `Item/RootInteractions`, indexed
//! (spec 07). Almost all of them are contained (written inline in items,
//! blocks, NPC roles…). Inheritance follows the schema.
//!
//! Built in, at indexes 0-5: `*PickupItem`, `*Simple_Crafting_Default`,
//! `*Diagram_Crafting_Default`, `*Structural_Crafting_Default`, `*UseNPC`
//! (each running the interaction of the same key) and `*Default_Swap`
//! (running `*Change_Active_Slot`, with cooldown `ChangeActiveSlot`: 0 s,
//! charges `[0]`, skip cooldown reset).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | key | `id` | |
//! | `Interactions` (keys or inline) | `interactions` (interaction indexes) | empty |
//! | `Cooldown` `{Id, Cooldown, Charges, SkipCooldownReset, InterruptRecharge, ClickBypass}` | `cooldown` | absent |
//! | `Settings` `{game mode: {Cooldown, AllowSkipChainOnClick}}` | `settings` | empty |
//! | `Rules` `{BlockedBy, Blocking, InterruptedBy, Interrupting}` (interaction type sets, sent in enum order; `Blocking` empty → absent) and `…Bypass` (→ tag index, `i32::MIN` when absent) | `rules` | |
//! | `Tags` | `tags` | |
//! | `ClickQueuingTimeout` | same | 0 |
//! | `RequireNewClick` | same | false |
//! | `HudInputBindingEntry` | same | absent |

use aerie_protocol::proto::{
    GameMode, InteractionCooldown, InteractionRules, InteractionType, RootInteraction,
    RootInteractionSettings, UpdateRootInteractions,
};
use aerie_protocol::wire::Wire;
use serde_json::Value;

use super::music::{owner_of, reference};
use crate::values::Get;
use crate::{Result, ctx::Ctx, store::Store, tags};

pub const PATH: &str = "Item/RootInteractions";
pub const BUILTIN: &[&str] = &[
    "*PickupItem",
    "*Simple_Crafting_Default",
    "*Diagram_Crafting_Default",
    "*Structural_Crafting_Default",
    "*UseNPC",
    "*Default_Swap",
];
pub const INTERACTIONS: &str = "Item/Interactions";
/// Interactions built into the server.
pub const INTERACTIONS_BUILTIN: &[&str] = &[
    "*Change_Active_Slot",
    "*PickupItem",
    "*Simple_Crafting_Default",
    "*Diagram_Crafting_Default",
    "*Structural_Crafting_Default",
    "*UseNPC",
];

pub fn cooldown(v: &Value) -> InteractionCooldown {
    InteractionCooldown {
        cooldown_id: v.str_at("Id"),
        cooldown: v.f32_or("Cooldown", 0.0),
        click_bypass: v.bool_or("ClickBypass", false),
        charge_times: v.get("Charges").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(Value::as_f64)
                .map(|x| x as f32)
                .collect()
        }),
        skip_cooldown_reset: v.bool_or("SkipCooldownReset", false),
        interrupt_recharge: v.bool_or("InterruptRecharge", false),
    }
}

fn type_set(v: &Value, key: &str) -> Option<Vec<InteractionType>> {
    let a = v.get(key)?.as_array()?;
    let mut t: Vec<InteractionType> = a
        .iter()
        .filter_map(|x| x.as_str().and_then(InteractionType::from_name))
        .collect();
    t.sort_by_key(|x| x.0);
    t.dedup();
    Some(t)
}

pub fn rules(ctx: &Ctx, v: Option<&Value>) -> InteractionRules {
    let null = Value::Null;
    let v = v.unwrap_or(&null);
    let bypass = |k: &str| v.str_at(k).map_or(i32::MIN, |t| ctx.tag(&t));
    InteractionRules {
        blocked_by: type_set(v, "BlockedBy"),
        blocking: type_set(v, "Blocking").filter(|b| !b.is_empty()),
        interrupted_by: type_set(v, "InterruptedBy"),
        interrupting: type_set(v, "Interrupting"),
        blocked_by_bypass_index: bypass("BlockedByBypass"),
        blocking_bypass_index: bypass("BlockingBypass"),
        interrupted_by_bypass_index: bypass("InterruptedByBypass"),
        interrupting_bypass_index: bypass("InterruptingBypass"),
    }
}

/// The interaction keys a root runs, in order.
pub fn interaction_keys(store: &Store, k: &str, a: &Value) -> Vec<String> {
    let owner = owner_of(store, k, "Interactions");
    a.get("Interactions")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .enumerate()
                .filter_map(|(i, x)| reference(x, &owner, &format!("Interactions_{i}")))
                .collect()
        })
        .unwrap_or_default()
}

fn root(ctx: &Ctx, store: &Store, k: &str) -> Result<RootInteraction> {
    let index = |key: &str| ctx.index_or(INTERACTIONS, INTERACTIONS_BUILTIN, Some(key), i32::MIN);
    if !store.own.contains_key(k) {
        let swap = k == "*Default_Swap";
        return Ok(RootInteraction {
            id: Some(k.to_owned()),
            interactions: vec![index(if swap { "*Change_Active_Slot" } else { k })?],
            cooldown: swap.then(|| InteractionCooldown {
                cooldown_id: Some("ChangeActiveSlot".into()),
                cooldown: 0.0,
                click_bypass: false,
                charge_times: Some(vec![0.0]),
                skip_cooldown_reset: true,
                interrupt_recharge: false,
            }),
            settings: Some(Vec::new()),
            rules: rules(ctx, None),
            tags: Some(Vec::new()),
            ..Default::default()
        });
    }
    let a = store.resolved(k, "RootInteraction");
    let interactions = interaction_keys(store, k, &a)
        .iter()
        .map(|x| index(x))
        .collect::<Result<Vec<_>>>()?;
    let settings = a
        .obj("Settings")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(mode, s)| {
                    Some((
                        GameMode::from_name(mode)?,
                        RootInteractionSettings {
                            allow_skip_chain_on_click: s.bool_or("AllowSkipChainOnClick", false),
                            cooldown: s.obj("Cooldown").map(cooldown),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(RootInteraction {
        id: Some(k.to_owned()),
        interactions,
        cooldown: a.obj("Cooldown").map(cooldown),
        settings: Some(settings),
        rules: rules(ctx, a.obj("Rules")),
        tags: Some(tags::indexes(ctx, store, k)),
        click_queuing_timeout: a.f32_or("ClickQueuingTimeout", 0.0),
        require_new_click: a.bool_or("RequireNewClick", false),
        hud_input_binding_entry: a.str_at("HudInputBindingEntry"),
    })
}

pub fn build(ctx: &Ctx) -> Result<UpdateRootInteractions> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    let mut list = Vec::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        list.push((i, root(ctx, &store, k)?));
    }
    Ok(UpdateRootInteractions {
        r#type: super::INIT,
        max_id: order.len() as i32,
        interactions: Some(list),
    })
}

/// Compare runs: the captured roots give the official indexes of the
/// interactions they run, their tag lists, and the tag indexes of their rule
/// bypasses.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    let (cap, _) = <UpdateRootInteractions as Wire>::decode(captured, 0)?;
    let store = ctx.store(PATH)?;
    let mut pairs = Vec::new();
    let mut obs = Vec::new();
    for (_, r) in cap.interactions.unwrap_or_default() {
        let Some(id) = r.id else { continue };
        let Some(k) = store.own.keys().find(|k| k.eq_ignore_ascii_case(&id)) else {
            if let Some(b) = BUILTIN.iter().find(|b| b.eq_ignore_ascii_case(&id)) {
                let target = if *b == "*Default_Swap" {
                    "*Change_Active_Slot"
                } else {
                    b
                };
                if let Some(i) = r.interactions.first() {
                    pairs.push((target.to_string(), *i));
                }
            }
            continue;
        };
        let a = store.resolved(k, "RootInteraction");
        for (key, i) in interaction_keys(&store, k, &a)
            .into_iter()
            .zip(r.interactions.iter())
        {
            pairs.push((key, *i));
        }
        if let Some(t) = r.tags {
            obs.push((tags::of(&store, k).expanded, t));
        }
        if let Some(rv) = a.obj("Rules") {
            for (key, i) in [
                ("BlockedByBypass", r.rules.blocked_by_bypass_index),
                ("BlockingBypass", r.rules.blocking_bypass_index),
                ("InterruptedByBypass", r.rules.interrupted_by_bypass_index),
                ("InterruptingBypass", r.rules.interrupting_bypass_index),
            ] {
                if let Some(t) = rv.str_at(key) {
                    ctx.pin_tag(&t, i);
                }
            }
        }
    }
    ctx.pin(INTERACTIONS, pairs);
    tags::learn(ctx, &obs);
    Ok(())
}

pub const UNARMED: &str = "Item/Unarmed/Interactions";

/// `UpdateUnarmedInteractions` (68): the `Empty` unarmed set's
/// `Interactions` (interaction type → root key or inline root), as root
/// interaction indexes; `SwapFrom` defaults to `*Default_Swap`.
pub fn build_unarmed(ctx: &Ctx) -> Result<aerie_protocol::proto::UpdateUnarmedInteractions> {
    let store = ctx.store(UNARMED)?;
    let mut out = Vec::new();
    let mut has_swap = false;
    if let Some(k) = store.own.keys().find(|k| k.eq_ignore_ascii_case("Empty")) {
        let a = store.resolved(k, "UnarmedInteractions");
        let owner = owner_of(&store, k, "Interactions");
        for (t, v) in a
            .obj("Interactions")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let Some(ty) = InteractionType::from_name(t) else {
                continue;
            };
            let Some(key) = reference(v, &owner, &format!("Interactions_{t}")) else {
                continue;
            };
            has_swap |= ty == InteractionType::SWAP_FROM;
            out.push((ty, ctx.index_or(PATH, BUILTIN, Some(&key), i32::MIN)?));
        }
    }
    if !has_swap {
        out.push((
            InteractionType::SWAP_FROM,
            ctx.index_or(PATH, BUILTIN, Some("*Default_Swap"), i32::MIN)?,
        ));
    }
    Ok(aerie_protocol::proto::UpdateUnarmedInteractions {
        r#type: super::INIT,
        interactions: Some(out),
    })
}
