//! `UpdateItems` (54): every item by id (spec 07), from `Item/Items` files
//! and contained items (`State` entries, inheriting from their owner).
//!
//! | JSON | Packet | Default |
//! | --- | --- | --- |
//! | key | `id` | |
//! | `Icon`, `Model`, `Animation`, `Set`, `SubCategory`, `DroppedItemAnimation` | same | absent |
//! | `Texture` | `texture` | `Items/Unknown.png` |
//! | `Scale` | `scale` | 1 |
//! | `PlayerAnimationsId` | same | `Default` |
//! | `UsePlayerAnimations`, `Consumable`, `Variant`, `ClipsGeometry`, `RenderDeployablePreview` | same | false |
//! | `MaxStack` | `max_stack` | -1 → 1 for tools, weapons, armor, builder and block selector tools, else 100 |
//! | `Reticle` (key or inline) | `reticle_index` | 0 |
//! | `IconProperties` `{Scale, Translation [x,y], Rotation [x,y,z]}` | same | absent |
//! | `TranslationProperties` `{Name, Description}` | same | absent |
//! | `ItemLevel` | same | 0 |
//! | `Quality` (not inherited) | `quality_index` | 0 |
//! | `ResourceTypes` `[{Id, Quantity}]` | same | absent when empty |
//! | `BlockType` | `block_id`: block index of the nearest item in the chain that writes one | 0 (none) |
//! | `Tool`, `Weapon`, `Armor`, `Glider`, `Utility`, `BlockSelectorTool`, `BuilderTool` | same | absent (`Utility`: always sent) |
//! | `ItemEntity` | `item_entity`: the quality's `ItemEntityConfig` when `Quality` is set | `{ParticleSystemId: Item}` (none for blocks), particles shown |
//! | `Categories` | same | absent when empty |
//! | `Particles`, `FirstPersonParticles`, `Trails` | same | absent when empty |
//! | `Light` | same | absent |
//! | `MaxDurability` | `durability` | 0 |
//! | `SoundEventId` | `sound_event_index` | 0 |
//! | `ItemSoundSetId` | `item_sound_set_index` | `ISS_Default` |
//! | `Interactions`, `CarryInteractions` (type → root key or inline) | same, root indexes, combined down the chain | empty |
//! | `InteractionVars` (name → root) | same | empty |
//! | `CarryHudInputBindings` | same | empty |
//! | `InteractionConfig` | same | outlines shown (hidden for weapons) |
//! | `Tags` | `tag_indexes` | |
//! | `ItemAppearanceConditions` (stat → list) | same, keyed by stat index | absent |
//! | `DisplayEntityStatsHUD` (stat names) | stat indexes | absent |
//! | `PullbackConfig` | `pullback_config` | absent |
//! | `HudUI` `[{Path, Type}]` | `hud_ui` | absent when empty |
//!
//! Missing interaction types are filled from the unarmed set named by
//! `PlayerAnimationsId`, then from `Empty`.

use aerie_protocol::proto as p;
use aerie_protocol::wire::Wire;
use serde_json::Value;

use super::music::{owner_of, reference};
use super::weapon_damage;
use super::{
    ability_keywords, blocks, entity_stat_types, fluids, item_animations, item_qualities,
    item_reticles, model_particles, root_interactions, sound_events, sound_sets,
};
use crate::chm::int_set_order;
use crate::ctx::{Ctx, enum_or};
use crate::store::Store;
use crate::values::{Get, direction, vec3};
use crate::{Result, tags};

pub const PATH: &str = "Item/Items";

fn arr<'a>(v: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    v.get(key)
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
}

fn floats<const N: usize>(v: Option<&Value>) -> Option<[f32; N]> {
    let a = v?.as_array()?;
    let mut out = [0.0; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = a.get(i)?.as_f64()? as f32;
    }
    Some(out)
}

fn stat(ctx: &Ctx, name: &str) -> Result<i32> {
    ctx.index_or(
        entity_stat_types::PATH,
        entity_stat_types::BUILTIN,
        Some(name),
        i32::MIN,
    )
}

fn stats(ctx: &Ctx, v: &Value, key: &str) -> Result<Option<Vec<i32>>> {
    match v.strings(key) {
        Some(s) => Ok(Some(s.iter().map(|n| stat(ctx, n)).collect::<Result<_>>()?)),
        None => Ok(None),
    }
}

fn modifier(m: &Value) -> p::Modifier {
    p::Modifier {
        target: enum_or(
            m,
            "Target",
            p::ModifierTarget::from_name,
            p::ModifierTarget::MAX,
        ),
        calculation_type: enum_or(
            m,
            "CalculationType",
            p::CalculationType::from_name,
            p::CalculationType::ADDITIVE,
        ),
        amount: m.f32_or("Amount", 0.0),
    }
}

fn modifiers(v: &Value) -> Vec<p::Modifier> {
    v.as_array()
        .map(|a| a.iter().map(modifier).collect())
        .unwrap_or_default()
}

/// `{stat: [modifier]}` keyed by stat index.
fn stat_modifiers(ctx: &Ctx, v: &Value) -> Result<Option<Vec<(i32, Vec<p::Modifier>)>>> {
    let Some(m) = v.get("StatModifiers").and_then(Value::as_object) else {
        return Ok(None);
    };
    m.iter()
        .map(|(k, l)| Ok((stat(ctx, k)?, modifiers(l))))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn named_modifiers(v: &Value, key: &str) -> Option<Vec<(String, Vec<p::Modifier>)>> {
    let m = v.get(key)?.as_object()?;
    (!m.is_empty()).then(|| m.iter().map(|(k, l)| (k.clone(), modifiers(l))).collect())
}

fn tool(t: &Value) -> p::ItemTool {
    p::ItemTool {
        specs: arr(t, "Specs").map(|a| {
            a.iter()
                .map(|s| p::ItemToolSpec {
                    gather_type: s.str_at("GatherType"),
                    power: s.f32_or("Power", 0.0),
                    quality: s.i32_or("Quality", 0),
                })
                .collect()
        }),
        speed: t.f32_or("Speed", 0.0),
        break_shape: t.obj("BreakShape").map(|b| {
            let off = b.obj("Offset");
            let c = |k: &str| off.map_or(0.0, |o| o.f32_or(k, 0.0));
            let width = b.i32_or("Width", 1);
            let height = b.i32_or("Height", 1);
            let depth = b.i32_or("Depth", 1);
            let centered = b.bool_or("Centered", true);
            let offset = Some([c("X"), c("Y"), c("Z")]);
            let orientation = enum_or(
                b,
                "Orientation",
                p::BreakShapeOrientation::from_name,
                p::BreakShapeOrientation::default(),
            );
            if b.str_at("Id").as_deref() == Some("Cylinder") {
                p::BreakShape::CylinderBreakShape(Box::new(p::CylinderBreakShape {
                    width,
                    height,
                    depth,
                    centered,
                    offset,
                    orientation,
                }))
            } else {
                p::BreakShape::BoxBreakShape(Box::new(p::BoxBreakShape {
                    width,
                    height,
                    depth,
                    centered,
                    offset,
                    orientation,
                }))
            }
        }),
    }
}

fn weapon(ctx: &Ctx, w: &Value) -> Result<p::ItemWeapon> {
    Ok(p::ItemWeapon {
        entity_stats_to_clear: stats(ctx, w, "EntityStatsToClear")?,
        stat_modifiers: stat_modifiers(ctx, w)?,
        render_dual_wielded: w.bool_or("RenderDualWielded", false),
        basic_damage_breakdown: None,
        ultimate_damage_breakdown: None,
    })
}

fn armor(ctx: &Ctx, a: &Value) -> Result<p::ItemArmor> {
    Ok(p::ItemArmor {
        armor_slot: enum_or(
            a,
            "ArmorSlot",
            p::ItemArmorSlot::from_name,
            p::ItemArmorSlot::HEAD,
        ),
        cosmetics_to_hide: a.get("CosmeticsToHide").and_then(Value::as_array).map(|l| {
            l.iter()
                .filter_map(|c| c.as_str().and_then(p::Cosmetic::from_name))
                .collect()
        }),
        stat_modifiers: stat_modifiers(ctx, a)?,
        base_damage_resistance: a.f64_or("BaseDamageResistance", 0.0),
        damage_resistance: a
            .get("DamageResistance")
            .and_then(Value::as_object)
            .filter(|m| !m.is_empty())
            .map(|m| {
                m.iter()
                    .map(|(k, l)| {
                        (
                            k.clone(),
                            l.as_array()
                                .into_iter()
                                .flatten()
                                .map(|r| p::ResistanceModifier {
                                    calculation_type: enum_or(
                                        r,
                                        "CalculationType",
                                        p::ResistanceCalculationType::from_name,
                                        p::ResistanceCalculationType::default(),
                                    ),
                                    amount: r.f32_or("Amount", 0.0),
                                })
                                .collect(),
                        )
                    })
                    .collect()
            }),
        damage_enhancement: named_modifiers(a, "DamageEnhancement"),
        damage_class_enhancement: a
            .get("DamageClassEnhancement")
            .and_then(Value::as_object)
            .filter(|m| !m.is_empty())
            .map(|m| {
                m.iter()
                    .map(|(k, l)| (k.to_lowercase(), modifiers(l)))
                    .collect()
            }),
        movement_settings: match a.obj("MovementSettings") {
            Some(m) => Some(p::ItemMovementSettings {
                extra_jump_count: m.i32_or("ExtraJumpCount", 0),
                extra_jump_particle_system: m.str_at("ExtraJumpParticleSystem"),
                extra_jump_sound_event_index: sound_events::index(
                    ctx,
                    m.str_at("ExtraJumpSoundEvent").as_deref(),
                )?,
                extra_jump_local_sound_event_index: sound_events::index(
                    ctx,
                    m.str_at("ExtraJumpLocalSoundEvent").as_deref(),
                )?,
            }),
            None => None,
        },
    })
}

fn keywords(ctx: &Ctx, v: &Value, key: &str) -> Result<Option<Vec<i32>>> {
    let Some(ids) = v.strings(key).filter(|i| !i.is_empty()) else {
        return Ok(None);
    };
    let mut idx = Vec::new();
    for id in &ids {
        let i = ctx.index_or(ability_keywords::PATH, &[], Some(id), i32::MIN)?;
        if i != i32::MIN && !idx.contains(&i) {
            idx.push(i);
        }
    }
    Ok((!idx.is_empty()).then(|| int_set_order(&idx, ids.len())))
}

/// `Ability`: `Core` (cooldown, cost, weapon families, keywords, cast root)
/// or `Support` (keyword requirements, on-hit effects, attribute modifiers).
fn ability(ctx: &Ctx, store: &Store, k: &str, v: &Value) -> Result<p::ItemAbility> {
    let slot = enum_or(v, "Slot", p::AbilitySlot::from_name, p::AbilitySlot::CORE);
    if slot == p::AbilitySlot::SUPPORT {
        return Ok(p::ItemAbility {
            slot,
            cost_type: p::AbilityCostType::NONE,
            requires_any_ability_keyword_indexes: keywords(ctx, v, "RequiresAnyAbilityKeyword")?,
            requires_all_ability_keyword_indexes: keywords(ctx, v, "RequiresAllAbilityKeywords")?,
            excludes_any_ability_keyword_indexes: keywords(ctx, v, "ExcludesAnyAbilityKeyword")?,
            on_hit_effects: v.strings("OnHitEffects"),
            attribute_modifiers: v
                .get("AttributeModifiers")
                .and_then(Value::as_object)
                .filter(|m| !m.is_empty())
                .map(|m| m.iter().map(|(k, l)| (k.clone(), modifiers(l))).collect()),
            cast: i32::MIN,
            ..Default::default()
        });
    }
    let weapons = v.strings("Weapons").filter(|w| !w.is_empty());
    let cast = match v.get("Cast") {
        Some(c) => reference(c, &owner_of(store, k, "Ability"), "Ability_Cast"),
        None => None,
    };
    Ok(p::ItemAbility {
        slot,
        cooldown: v.f32_or("Cooldown", 0.0),
        cost: v.f32_or("Cost", 0.0),
        cost_type: enum_or(
            v,
            "CostType",
            p::AbilityCostType::from_name,
            p::AbilityCostType::MANA,
        ),
        weapon_tag_indexes: weapons.as_ref().map(|w| {
            w.iter()
                .map(|f| ctx.tag(&format!("Family={}", f.trim())))
                .collect()
        }),
        weapons,
        ability_keyword_indexes: keywords(ctx, v, "AbilityKeywords")?,
        attributes: v.get("Attributes").and_then(Value::as_object).map(|m| {
            m.iter()
                .filter_map(|(k, x)| Some((k.clone(), x.as_f64()? as f32)))
                .collect()
        }),
        cast: root_index(ctx, cast.as_deref())?,
        ..Default::default()
    })
}

/// `{name: translation key}` as translated messages; absent when empty.
fn message_args(t: &Value, key: &str) -> Option<Vec<(String, p::FormattedMessage)>> {
    let m = t.get(key)?.as_object()?;
    (!m.is_empty()).then(|| {
        m.iter()
            .filter_map(|(k, v)| {
                Some((
                    k.clone(),
                    p::FormattedMessage {
                        message_id: Some(v.as_str()?.to_owned()),
                        ..Default::default()
                    },
                ))
            })
            .collect()
    })
}

/// A block pattern as `BlockPattern.toString` prints it: a single block
/// alone, else `chance%block` joined by commas (chance 100 when absent,
/// repeats kept).
fn block_pattern(s: &str) -> String {
    if s.is_empty() || s == "Empty" {
        return "Empty".into();
    }
    let mut entries: Vec<(String, f64)> = Vec::new();
    for part in s.replace(';', ",").split(',').filter(|p| !p.is_empty()) {
        let (chance, block) = match part.split_once('%') {
            Some((c, b)) => (c.parse().unwrap_or(100.0), b.to_owned()),
            None => (100.0, part.to_owned()),
        };
        entries.push((block, chance));
    }
    if entries.len() == 1 {
        return entries.remove(0).0;
    }
    entries
        .iter()
        .map(|(b, c)| {
            let c = if c.fract() == 0.0 {
                format!("{c:.1}")
            } else {
                format!("{c}")
            };
            format!("{c}%{b}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// `BuilderTool.Args` by id, in the order first written (a later entry
/// with the same id replaces the earlier one in place).
fn builder_args(b: &Value) -> Vec<p::BuilderToolArg> {
    let mut out: Vec<p::BuilderToolArg> = Vec::new();
    for a in b
        .get("Args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut arg = p::BuilderToolArg {
            required: a.bool_or("Required", true),
            id: a.str_at("Id"),
            ..Default::default()
        };
        match a.str_at("Type").as_deref() {
            Some("Bool") => {
                arg.arg_type = p::BuilderToolArgType::BOOL;
                arg.bool_arg = Some(p::BuilderToolBoolArg {
                    default_value: a.bool_or("Default", false),
                });
            }
            Some("Int") => {
                arg.arg_type = p::BuilderToolArgType::INT;
                arg.int_arg = Some(p::BuilderToolIntArg {
                    default_value: a.i32_or("Default", 0),
                    min: a.i32_or("Min", 0),
                    max: a.i32_or("Max", 0),
                });
            }
            Some("Float") => {
                arg.arg_type = p::BuilderToolArgType::FLOAT;
                arg.float_arg = Some(p::BuilderToolFloatArg {
                    default_value: a.f32_or("Default", 0.0),
                    min: a.f32_or("Min", 0.0),
                    max: a.f32_or("Max", 0.0),
                });
            }
            Some("String") => {
                arg.arg_type = p::BuilderToolArgType::STRING;
                arg.string_arg = Some(p::BuilderToolStringArg {
                    default_value: a.str_at("Default"),
                });
            }
            Some("Block") => {
                arg.arg_type = p::BuilderToolArgType::BLOCK;
                arg.block_arg = Some(p::BuilderToolBlockArg {
                    default_value: Some(block_pattern(&a.str_at("Default").unwrap_or_default())),
                    allow_pattern: a.bool_or("AllowPattern", false),
                });
            }
            Some("Mask") => {
                arg.arg_type = p::BuilderToolArgType::MASK;
                arg.mask_arg = Some(p::BuilderToolMaskArg {
                    default_value: a.str_at("Default"),
                });
            }
            Some("Option") => {
                arg.arg_type = p::BuilderToolArgType::OPTION;
                arg.option_arg = Some(p::BuilderToolOptionArg {
                    default_value: a.str_at("Default"),
                    options: a.strings("Options"),
                });
            }
            _ => continue,
        }
        match out.iter_mut().find(|x| x.id == arg.id) {
            Some(x) => *x = arg,
            None => out.push(arg),
        }
    }
    out
}

fn glider(g: &Value) -> p::ItemGlider {
    p::ItemGlider {
        terminal_velocity: g.f32_or("TerminalVelocity", 0.0),
        fall_speed_multiplier: g.f32_or("FallSpeedMultiplier", 0.0),
        horizontal_speed_multiplier: g.f32_or("HorizontalSpeedMultiplier", 0.0),
        speed: g.f32_or("Speed", 0.0),
        dive_terminal_velocity: g.f32_or("DiveTerminalVelocity", 0.0),
        dive_speed: g.f32_or("DiveSpeed", 0.0),
        dive_fall_speed_multiplier: g.f32_or("DiveFallSpeedMultiplier", 0.0),
        dive_horizontal_speed_multiplier: g.f32_or("DiveHorizontalSpeedMultiplier", 0.0),
        energy_climb_rate: g.f32_or("EnergyClimbRate", 0.0),
        dive_speed_to_energy_ratio: g.f32_or("DiveSpeedToEnergyRatio", 0.0),
    }
}

fn utility(ctx: &Ctx, u: Option<&Value>) -> Result<p::ItemUtility> {
    let null = Value::Null;
    let u = u.unwrap_or(&null);
    Ok(p::ItemUtility {
        usable: u.bool_or("Usable", false),
        compatible: u.bool_or("Compatible", false),
        entity_stats_to_clear: stats(ctx, u, "EntityStatsToClear")?,
        stat_modifiers: stat_modifiers(ctx, u)?,
    })
}

fn item_entity(v: Option<&Value>, block: bool) -> p::ItemEntityConfig {
    match v {
        Some(e) => p::ItemEntityConfig {
            particle_system_id: match e.get("ParticleSystemId") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => None,
                None => Some("Item".into()),
            },
            particle_color: e.color("ParticleColor"),
            show_item_particles: e.bool_or("ShowItemParticles", true),
        },
        None => p::ItemEntityConfig {
            particle_system_id: (!block).then(|| "Item".into()),
            particle_color: None,
            show_item_particles: true,
        },
    }
}

/// An item's entity config after loading: its quality's when it names one,
/// else its own `ItemEntity`, else its parent's, else the default.
fn item_entity_of(ctx: &Ctx, store: &Store, k: &str, depth: usize) -> Result<p::ItemEntityConfig> {
    let own = &store.own[k];
    let block = block_owner(store, k).is_some();
    let cfg = if let Some(q) = own.str_at("Quality") {
        let i = ctx.index_or(item_qualities::PATH, item_qualities::BUILTIN, Some(&q), 0)?;
        let qs = ctx.store(item_qualities::PATH)?;
        let order = ctx.order(item_qualities::PATH, item_qualities::BUILTIN)?;
        order
            .get(i as usize)
            .filter(|q| qs.own.contains_key(*q))
            .and_then(|q| {
                qs.resolved(q, "ItemQuality")
                    .get("ItemEntityConfig")
                    .cloned()
            })
            .map(|v| item_entity(Some(&v), block))
    } else if let Some(e) = own.get("ItemEntity") {
        Some(item_entity(Some(e), block))
    } else {
        match store.parents.get(k).filter(|p| store.own.contains_key(*p)) {
            Some(p) if depth < 32 => Some(item_entity_of(ctx, store, p, depth + 1)?),
            _ => None,
        }
    };
    Ok(cfg.unwrap_or_else(|| item_entity(None, block)))
}

/// The nearest item in the chain that writes a `BlockType`.
fn block_owner(store: &Store, k: &str) -> Option<String> {
    chain(store, k).into_iter().find(|c| {
        store
            .own
            .get(c)
            .is_some_and(|o| o.get("BlockType").is_some())
    })
}

fn interaction_config(c: Option<&Value>, weapon: bool) -> p::InteractionConfiguration {
    let null = Value::Null;
    let c = c.unwrap_or(&null);
    p::InteractionConfiguration {
        display_outlines: c.bool_or("DisplayOutlines", !weapon || c.is_object()),
        debug_outlines: c.bool_or("DebugOutlines", false),
        // A written config starts from the default distances.
        use_distance: c.is_object().then(|| {
            let mut d = vec![(p::GameMode::ADVENTURE, 5.0), (p::GameMode::CREATIVE, 6.0)];
            for (k, v) in c
                .get("UseDistance")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if let (Some(m), Some(v)) = (p::GameMode::from_name(k), v.as_f64()) {
                    d.retain(|(x, _)| *x != m);
                    d.push((m, v as f32));
                }
            }
            d
        }),
        all_entities: c.bool_or("AllEntities", false),
        priorities: c
            .get("Priorities")
            .and_then(Value::as_object)
            .filter(|m| !m.is_empty())
            .map(|m| {
                m.iter()
                    .filter_map(|(t, v)| {
                        let values = match v {
                            Value::Object(o) => o
                                .iter()
                                .filter_map(|(s, n)| {
                                    Some((p::PrioritySlot::from_name(s)?, n.as_i64()? as i32))
                                })
                                .collect(),
                            _ => vec![(p::PrioritySlot::DEFAULT, v.as_i64()? as i32)],
                        };
                        Some((
                            p::InteractionType::from_name(t)?,
                            p::InteractionPriority {
                                values: Some(values),
                            },
                        ))
                    })
                    .collect()
            }),
    }
}

/// The chain from `k` up to the root (`k` first).
fn chain(store: &Store, k: &str) -> Vec<String> {
    let mut out = vec![k.to_owned()];
    let mut c = store.parents.get(k);
    while let Some(pk) = c {
        if out.len() > 32 || !store.own.contains_key(pk) || out.contains(pk) {
            break;
        }
        out.push(pk.clone());
        c = store.parents.get(pk);
    }
    out
}

/// A map field combined down the chain: (entry, value, asset that wrote it).
fn combined(store: &Store, chain: &[String], field: &str) -> Vec<(String, Value, String)> {
    let mut out: Vec<(String, Value, String)> = Vec::new();
    for owner in chain.iter().rev() {
        if let Some(Value::Object(m)) = store.own.get(owner).and_then(|o| o.get(field)) {
            for (t, v) in m {
                out.retain(|(x, _, _)| x != t);
                if !v.is_null() {
                    out.push((t.clone(), v.clone(), owner.clone()));
                }
            }
        }
    }
    out
}

fn root_index(ctx: &Ctx, key: Option<&str>) -> Result<i32> {
    ctx.index_or(
        root_interactions::PATH,
        root_interactions::BUILTIN,
        key,
        i32::MIN,
    )
}

fn typed_roots(
    ctx: &Ctx,
    entries: &[(String, Value, String)],
    field: &str,
) -> Result<Vec<(p::InteractionType, i32)>> {
    let mut out = Vec::new();
    for (t, v, owner) in entries {
        let Some(ty) = p::InteractionType::from_name(t) else {
            continue;
        };
        let key = reference(v, owner, &format!("{field}_{t}"));
        out.push((ty, root_index(ctx, key.as_deref())?));
    }
    Ok(out)
}

fn item(ctx: &Ctx, store: &Store, unarmed: &Store, k: &str) -> Result<p::ItemBase> {
    let a = store.resolved(k, "Item");
    let chain = chain(store, k);
    let own = &store.own[k];

    let block_id = match block_owner(store, k) {
        Some(b) => ctx.index_or(blocks::PATH, blocks::BUILTIN, Some(&b), 1)?,
        None => 0,
    };

    let tool_v = a.obj("Tool");
    let weapon_v = a.obj("Weapon");
    let armor_v = a.obj("Armor");
    let builder_v = a.obj("BuilderTool");
    let selector_v = a.obj("BlockSelectorTool");
    let mut max_stack = a.i32_or("MaxStack", -1);
    if max_stack == -1 {
        max_stack = if tool_v.is_none()
            && weapon_v.is_none()
            && armor_v.is_none()
            && builder_v.is_none()
            && selector_v.is_none()
        {
            100
        } else {
            1
        };
    }

    let reticle_index = match a.get("Reticle") {
        Some(r) => {
            let owner = owner_of(store, k, "Reticle");
            let key = reference(r, &owner, "Reticle");
            ctx.index_or(
                item_reticles::PATH,
                item_reticles::BUILTIN,
                key.as_deref(),
                0,
            )?
        }
        None => 0,
    };

    let quality = own.str_at("Quality");
    let _ = &own;
    let quality_index = ctx.index_or(
        item_qualities::PATH,
        item_qualities::BUILTIN,
        quality.as_deref(),
        0,
    )?;
    let entity = item_entity_of(ctx, store, k, 0)?;

    // Interactions, with the unarmed fallbacks.
    let mut inter = combined(store, &chain, "Interactions");
    let player_anim = match a.get("PlayerAnimationsId") {
        Some(Value::Object(_)) => {
            // Inline: keyed after the item itself when that key exists, else
            // after the item that wrote it.
            let anims = ctx.store(item_animations::PATH)?;
            let mine = format!("*{k}_PlayerAnimationsId");
            if anims.own.contains_key(&mine) {
                mine
            } else {
                format!(
                    "*{}_PlayerAnimationsId",
                    owner_of(store, k, "PlayerAnimationsId")
                )
            }
        }
        Some(Value::String(s)) => s.clone(),
        _ => "Default".into(),
    };
    let _ = &mut inter;
    let inter = final_interactions(store, unarmed, k, 0);
    let mut interactions = typed_roots(ctx, &inter, "Interactions")?;
    if !interactions
        .iter()
        .any(|(t, _)| *t == p::InteractionType::SWAP_FROM)
    {
        interactions.push((
            p::InteractionType::SWAP_FROM,
            root_index(ctx, Some("*Default_Swap"))?,
        ));
    }
    let carry = typed_roots(
        ctx,
        &combined(store, &chain, "CarryInteractions"),
        "CarryInteractions",
    )?;
    let vars = combined(store, &chain, "InteractionVars")
        .iter()
        .map(|(n, v, owner)| {
            let key = reference(v, owner, &format!("InteractionVars_{n}"));
            Ok((n.clone(), root_index(ctx, key.as_deref())?))
        })
        .collect::<Result<Vec<_>>>()?;
    let hud_bindings = combined(store, &chain, "CarryHudInputBindings")
        .into_iter()
        .filter_map(|(t, v, _)| Some((p::InteractionType::from_name(&t)?, v.as_str()?.to_owned())))
        .collect();

    let appearance = match a.get("ItemAppearanceConditions").and_then(Value::as_object) {
        Some(m) => {
            let mut out = Vec::new();
            for (s, l) in m {
                let conds = l
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        let world =
                            sound_events::index(ctx, c.str_at("WorldSoundEventId").as_deref())?;
                        let local =
                            sound_events::index(ctx, c.str_at("LocalSoundEventId").as_deref())?;
                        Ok(p::ItemAppearanceCondition {
                            particles: arr(c, "Particles")
                                .map(|a| a.iter().map(model_particles::one).collect()),
                            first_person_particles: arr(c, "FirstPersonParticles")
                                .map(|a| a.iter().map(model_particles::one).collect()),
                            model: c.str_at("Model"),
                            texture: c.str_at("Texture"),
                            model_vfx_id: c.str_at("ModelVFXId"),
                            condition: c.get("Condition").and_then(|r| {
                                let a = r.as_array()?;
                                Some(p::FloatRange {
                                    inclusive_min: a.first()?.as_f64()? as f32,
                                    inclusive_max: a.get(1)?.as_f64()? as f32,
                                })
                            }),
                            condition_value_type: enum_or(
                                c,
                                "ConditionValueType",
                                p::ValueType::from_name,
                                p::ValueType::ABSOLUTE,
                            ),
                            local_sound_event_id: if local != 0 { local } else { world },
                            world_sound_event_id: world,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                out.push((stat(ctx, s)?, conds));
            }
            Some(out)
        }
        None => None,
    };

    let tag_names = tags::of(store, k);
    let tag_indexes = tags::indexes(ctx, store, k);
    let _ = tag_names;

    Ok(p::ItemBase {
        id: Some(k.to_owned()),
        model: a.str_at("Model"),
        scale: a.f32_or("Scale", 1.0),
        texture: Some(
            a.str_at("Texture")
                .unwrap_or_else(|| "Items/Unknown.png".into()),
        ),
        animation: a.str_at("Animation"),
        player_animations_id: Some(player_anim),
        use_player_animations: a.bool_or("UsePlayerAnimations", false),
        max_stack,
        reticle_index,
        icon: a.str_at("Icon"),
        icon_properties: a.obj("IconProperties").map(|i| p::AssetIconProperties {
            scale: i.f32_or("Scale", 0.0),
            translation: floats(i.get("Translation")),
            rotation: floats(i.get("Rotation")),
        }),
        translation_properties: a.obj("TranslationProperties").map(|t| {
            p::ItemTranslationProperties {
                name: t.str_at("Name"),
                name_arguments: message_args(t, "NameArguments"),
                description: t.str_at("Description"),
                description_arguments: message_args(t, "DescriptionArguments"),
            }
        }),
        item_level: a.i32_or("ItemLevel", 0),
        quality_index,
        resource_types: arr(&a, "ResourceTypes").map(|l| {
            l.iter()
                .map(|r| p::ItemResourceType {
                    id: r.str_at("Id"),
                    quantity: r.i32_or("Quantity", 1),
                })
                .collect()
        }),
        consumable: a.bool_or("Consumable", false),
        variant: a.bool_or("Variant", false),
        block_id,
        tool: tool_v.map(tool),
        weapon: match weapon_v {
            Some(w) => {
                let mut wp = weapon(ctx, w)?;
                let root_of = |t: &str| {
                    inter
                        .iter()
                        .find(|(x, _, _)| x == t)
                        .and_then(|(t, v, o)| reference(v, o, &format!("Interactions_{t}")))
                };
                let var_roots: std::collections::HashMap<String, String> =
                    combined(store, &chain, "InteractionVars")
                        .iter()
                        .filter_map(|(n, v, o)| {
                            Some((n.clone(), reference(v, o, &format!("InteractionVars_{n}"))?))
                        })
                        .collect();
                wp.basic_damage_breakdown = Some(weapon_damage::breakdown(
                    ctx,
                    root_of("Primary").as_deref(),
                    &var_roots,
                )?);
                wp.ultimate_damage_breakdown = Some(weapon_damage::breakdown(
                    ctx,
                    root_of("Ability1").as_deref(),
                    &var_roots,
                )?);
                Some(wp)
            }
            None => None,
        },
        armor: armor_v.map(|x| armor(ctx, x)).transpose()?,
        glider_config: a.obj("Glider").map(glider),
        utility: Some(utility(ctx, a.obj("Utility"))?),
        block_selector_tool: selector_v.map(|s| p::BlockSelectorToolData {
            durability_loss_on_use: s.f32_or("DurabilityLossOnUse", 0.0),
        }),
        builder_tool_data: builder_v.map(|b| p::BuilderToolState {
            id: b.str_at("Id"),
            is_brush: b.bool_or("IsBrush", false),
            args: Some(builder_args(b)),
        }),
        item_entity: Some(entity),
        set: a.str_at("Set"),
        categories: a.strings("Categories").filter(|c| !c.is_empty()),
        sub_category: a.str_at("SubCategory"),
        particles: arr(&a, "Particles").map(|l| l.iter().map(model_particles::one).collect()),
        first_person_particles: arr(&a, "FirstPersonParticles")
            .map(|l| l.iter().map(model_particles::one).collect()),
        trails: arr(&a, "Trails").map(|l| {
            l.iter()
                .map(|t| p::ModelTrail {
                    trail_id: t.str_at("TrailId"),
                    target_entity_part: enum_or(
                        t,
                        "TargetEntityPart",
                        p::EntityPart::from_name,
                        p::EntityPart::SELF,
                    ),
                    target_node_name: t.str_at("TargetNodeName"),
                    position_offset: t.obj("PositionOffset").map(vec3),
                    rotation_offset: t.obj("RotationOffset").map(direction),
                    fixed_rotation: t.bool_or("FixedRotation", false),
                })
                .collect()
        }),
        light: a.obj("Light").map(fluids::light),
        durability: a.f64_or("MaxDurability", 0.0),
        sound_event_index: sound_events::index(ctx, a.str_at("SoundEventId").as_deref())?,
        item_sound_set_index: ctx.index_or(
            sound_sets::ITEM,
            &[],
            Some(
                &a.str_at("ItemSoundSetId")
                    .unwrap_or_else(|| "ISS_Default".into()),
            ),
            i32::MIN,
        )?,
        interactions: Some(interactions),
        carry_interactions: Some(carry),
        carry_hud_input_bindings: Some(hud_bindings),
        interaction_vars: Some(vars),
        interaction_config: Some(interaction_config(
            a.obj("InteractionConfig"),
            weapon_v.is_some(),
        )),
        dropped_item_animation: a.str_at("DroppedItemAnimation"),
        tag_indexes: Some(tag_indexes),
        item_appearance_conditions: appearance,
        display_entity_stats_hud: stats(ctx, &a, "DisplayEntityStatsHUD")?,
        pullback_config: a
            .obj("PullbackConfig")
            .map(|c| p::ItemPullbackConfiguration {
                left_offset_override: floats(c.get("LeftOffsetOverride")),
                left_rotation_override: floats(c.get("LeftRotationOverride")),
                right_offset_override: floats(c.get("RightOffsetOverride")),
                right_rotation_override: floats(c.get("RightRotationOverride")),
            }),
        clips_geometry: a.bool_or("ClipsGeometry", false),
        render_deployable_preview: a.bool_or("RenderDeployablePreview", false),
        hud_ui: arr(&a, "HudUI").map(|l| {
            l.iter()
                .map(|h| p::ItemHudUI {
                    path: h.str_at("Path"),
                    r#type: enum_or(
                        h,
                        "Type",
                        p::ItemHudUIType::from_name,
                        p::ItemHudUIType::HUD,
                    ),
                })
                .collect()
        }),
        ability: a
            .obj("Ability")
            .map(|x| ability(ctx, store, k, x))
            .transpose()?,
    })
}

/// An item's interactions after loading: its parent's final map (fallbacks
/// included), then its own entries over it, then any type still missing
/// from the unarmed set named by its `PlayerAnimationsId`, then from
/// `Empty`. Entries are (type, value, asset that wrote it).
fn final_interactions(
    store: &Store,
    unarmed: &Store,
    k: &str,
    depth: usize,
) -> Vec<(String, Value, String)> {
    let mut out = match store.parents.get(k).filter(|p| store.own.contains_key(*p)) {
        Some(p) if depth < 32 => final_interactions(store, unarmed, p, depth + 1),
        _ => Vec::new(),
    };
    if let Some(Value::Object(m)) = store.own.get(k).and_then(|o| o.get("Interactions")) {
        for (t, v) in m {
            out.retain(|(x, _, _)| x != t);
            if !v.is_null() {
                out.push((t.clone(), v.clone(), k.to_owned()));
            }
        }
    }
    let set = match store.resolved(k, "Item").get("PlayerAnimationsId") {
        Some(Value::String(s)) => s.clone(),
        Some(_) => String::new(),
        None => "Default".into(),
    };
    for set in [set.as_str(), "Empty"] {
        let Some(uk) = unarmed.own.keys().find(|u| u.eq_ignore_ascii_case(set)) else {
            continue;
        };
        for (t, v, o) in combined(unarmed, &chain(unarmed, uk), "Interactions") {
            if !out.iter().any(|(x, _, _)| *x == t) {
                out.push((t, v, o));
            }
        }
    }
    out
}

/// The built-in `Unknown` item.
fn unknown(ctx: &Ctx) -> Result<p::ItemBase> {
    Ok(p::ItemBase {
        id: Some("Unknown".into()),
        model: Some("Items/CreativeTools/EditorTool.blockymodel".into()),
        scale: 1.0,
        texture: Some("Items/Unknown.png".into()),
        player_animations_id: Some("Item".into()),
        max_stack: 100,
        utility: Some(p::ItemUtility::default()),
        item_entity: Some(item_entity(None, false)),
        interactions: Some(vec![(
            p::InteractionType::SWAP_FROM,
            root_index(ctx, Some("*Default_Swap"))?,
        )]),
        carry_interactions: Some(Vec::new()),
        carry_hud_input_bindings: Some(Vec::new()),
        interaction_vars: Some(Vec::new()),
        interaction_config: Some(interaction_config(None, false)),
        ..Default::default()
    })
}

pub fn build(ctx: &Ctx) -> Result<p::UpdateItems> {
    let store = ctx.store(PATH)?;
    let unarmed = ctx.store(root_interactions::UNARMED)?;
    let mut list = Vec::new();
    for k in store.own.keys() {
        list.push((k.clone(), item(ctx, &store, &unarmed, k)?));
    }
    if !store.own.contains_key("Unknown") {
        list.push(("Unknown".into(), unknown(ctx)?));
    }
    Ok(p::UpdateItems {
        r#type: super::INIT,
        items: Some(list),
        removed_items: None,
        update_models: false,
        update_icons: false,
    })
}

/// Compare runs: learns tag indexes from the captured item tag lists.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    let (cap, _) = <p::UpdateItems as Wire>::decode(captured, 0)?;
    let store = ctx.store(PATH)?;
    let obs: Vec<(Vec<String>, Vec<i32>)> = cap
        .items
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, _)| store.own.contains_key(k))
        .map(|(k, i)| {
            (
                tags::of(&store, &k).expanded,
                i.tag_indexes.unwrap_or_default(),
            )
        })
        .collect();
    tags::learn(ctx, &obs);
    Ok(())
}
