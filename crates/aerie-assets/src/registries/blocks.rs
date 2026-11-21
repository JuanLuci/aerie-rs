//! `UpdateBlockTypes` (40) from `Item/Block/Blocks`, indexed (spec 07).
//! Nearly every block is contained: an item's `BlockType` (keyed like the
//! item), and block states (`State.Definitions.<name>`, keyed
//! `*<block>_State_Definitions_<name>`). Inheritance follows the schema;
//! defaults come from it too (`schema.field_default`).
//!
//! Notable rules (from the official decoder):
//! - `item`: the item the block is written in (through state blocks too).
//! - `group`: group ids are given in index order, as groups first appear.
//! - `support`: a block of material `Empty` outside group `@Tech` without
//!   `Support` needs a full face below; `supporting`: a solid cube (draw type
//!   `Cube`, `CubeWithModel` or `GizmoCube`) without `Supporting` supports all
//!   six faces fully.
//! - `interaction_hint`: `open` with a bench or `IsDoor`, `gather` when
//!   harvestable, `sit` with seats, else `generic`.
//! - `interactions`: combined by entry down the parent chain (a `null` entry
//!   removes one); a bench without `Use` gets its page's root.
//! - `states`: each state name → its block, plus `default` → the block a
//!   state block belongs to.
//! - tags: blocks written in an item (and states) start from their
//!   container's tags.
//! - tints: the first color of `Tint` / `TintUp`… as ARGB, else -1.

use aerie_protocol::proto::{self as p, InteractionType};
use serde_json::Value;

use super::music::reference;
use super::{
    block_hitboxes, fluids, materials, model_particles, root_interactions, sound_events, sound_sets,
};
use crate::ctx::{Ctx, enum_or};
use crate::schema;
use crate::store::Store;
use crate::values::Get;
use crate::{Result, tags};

pub const PATH: &str = "Item/Block/Blocks";
/// Built into the server, first in index order.
pub const BUILTIN: &[&str] = &["Empty", "Unknown", "Debug_Cube", "Debug_Model"];
pub const ITEMS: &str = "Item/Items";
/// Pin key of block group ids (compare runs).
pub const GROUPS: &str = "#block groups";

/// A field, or its schema default.
fn field<'a>(a: &'a Value, key: &str) -> Option<&'a Value> {
    a.get(key)
        .filter(|v| !v.is_null())
        .or_else(|| schema::get().field_default("BlockType", key))
}

fn s_or(a: &Value, key: &str) -> Option<String> {
    field(a, key).and_then(Value::as_str).map(str::to_owned)
}

fn f_or(a: &Value, key: &str, d: f32) -> f32 {
    field(a, key)
        .and_then(Value::as_f64)
        .map_or(d, |x| x as f32)
}

fn b_or(a: &Value, key: &str, d: bool) -> bool {
    field(a, key).and_then(Value::as_bool).unwrap_or(d)
}

fn e<T: Default>(a: &Value, key: &str, from: fn(&str) -> Option<T>) -> T {
    field(a, key)
        .and_then(Value::as_str)
        .and_then(from)
        .unwrap_or_default()
}

fn argb(c: &p::Color) -> i32 {
    (0xFFu32 << 24 | (c.red as u8 as u32) << 16 | (c.green as u8 as u32) << 8 | c.blue as u8 as u32)
        as i32
}

/// The faces a support key names: one face, or a merged set.
fn faces(key: &str) -> Vec<p::BlockNeighbor> {
    let n = |names: &[&str]| {
        names
            .iter()
            .filter_map(|f| p::BlockNeighbor::from_name(f))
            .collect()
    };
    match key {
        "All" => (0..26).map(p::BlockNeighbor).collect(),
        "BlockSides" => n(&["Up", "Down", "North", "East", "South", "West"]),
        "CardinalDirections" => n(&["North", "East", "South", "West"]),
        "Horizontal" => n(&[
            "North",
            "NorthEast",
            "East",
            "SouthEast",
            "South",
            "SouthWest",
            "West",
            "NorthWest",
        ]),
        "UpCardinalDirections" => n(&["UpNorth", "UpEast", "UpSouth", "UpWest"]),
        "DownCardinalDirections" => n(&["DownNorth", "DownEast", "DownSouth", "DownWest"]),
        f => p::BlockNeighbor::from_name(f).into_iter().collect(),
    }
}

/// A face map with merged keys: each face gets the lists of every key that
/// names it, in the order written.
fn by_face<T: Clone>(
    m: &serde_json::Map<String, Value>,
    mut one: impl FnMut(&Value) -> Result<T>,
) -> Result<Vec<(p::BlockNeighbor, Vec<T>)>> {
    let mut out: Vec<(p::BlockNeighbor, Vec<T>)> = Vec::new();
    for (key, list) in m {
        let Some(list) = list.as_array() else {
            continue;
        };
        let items = list.iter().map(&mut one).collect::<Result<Vec<_>>>()?;
        for f in faces(key) {
            match out.iter_mut().find(|(x, _)| *x == f) {
                Some((_, v)) => v.extend(items.iter().cloned()),
                None => out.push((f, items.clone())),
            }
        }
    }
    Ok(out)
}

fn vec3i(v: &Value) -> p::Vector3i {
    p::Vector3i {
        x: v.i32_or("X", 0),
        y: v.i32_or("Y", 0),
        z: v.i32_or("Z", 0),
    }
}

fn fillers(v: &Value) -> Option<Vec<p::Vector3i>> {
    v.get("Filler")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(vec3i).collect())
}

fn support_match(v: &Value, key: &str, d: &str) -> p::SupportMatch {
    p::SupportMatch::from_name(v.str_at(key).as_deref().unwrap_or(d)).unwrap_or_default()
}

/// The item a block is written in, following state blocks up.
fn item_of(store: &Store, key: &str) -> Option<String> {
    let mut k = key.to_owned();
    for _ in 0..16 {
        let (oc, ok, _) = store.contained_files.get(&k)?;
        if oc == "Item" {
            return Some(ok.clone());
        }
        if oc != "BlockType" {
            return None;
        }
        k = ok.clone();
    }
    None
}

/// The block a state block belongs to.
fn container_block(store: &Store, key: &str) -> Option<String> {
    let (oc, ok, _) = store.contained_files.get(key)?;
    (oc == "BlockType").then(|| ok.clone())
}

fn block(
    ctx: &Ctx,
    store: &Store,
    items: &Store,
    k: &str,
    groups: &mut Vec<Option<String>>,
) -> Result<p::BlockType> {
    block_from(ctx, store, items, k, store.resolved(k, "BlockType"), groups)
}

/// The built-in blocks as the server defines them, in the JSON form.
fn builtin_json(k: &str) -> Value {
    let tex = |f: &str| format!("BlockTextures/_Debug/{f}.png");
    match k {
        "Empty" => serde_json::json!({
            "DrawType": "Empty", "Material": "Empty", "Opacity": "Transparent",
            "Group": "Air", "Support": {}
        }),
        "Debug_Cube" => serde_json::json!({
            "DrawType": "Cube", "Material": "Solid", "VariantRotation": "Debug",
            "Textures": [{
                "Up": tex("Up"), "Down": tex("Down"), "North": tex("North"),
                "South": tex("South"), "East": tex("East"), "West": tex("West"), "Weight": 1
            }]
        }),
        "Debug_Model" => serde_json::json!({
            "DrawType": "Model", "Material": "Empty", "VariantRotation": "Debug",
            "CustomModel": "Blocks/_Debug/Model.blockymodel",
            "CustomModelTexture": [{"Texture": "Blocks/_Debug/Texture.png", "Weight": 1}]
        }),
        _ => serde_json::json!({"DrawType": "Cube", "Material": "Solid"}),
    }
}

/// The topmost ancestor of `k` in `store` (`k` itself without a parent).
fn root_of(store: &Store, k: &str) -> String {
    let mut cur = k.to_owned();
    for _ in 0..32 {
        match store
            .parents
            .get(&cur)
            .filter(|p| store.own.contains_key(*p))
        {
            Some(p) => cur = p.clone(),
            None => break,
        }
    }
    cur
}

fn block_from(
    ctx: &Ctx,
    store: &Store,
    items: &Store,
    k: &str,
    a: Value,
    groups: &mut Vec<Option<String>>,
) -> Result<p::BlockType> {
    let group = a.str_at("Group");
    let group_id = match (&group, ctx.pinned(GROUPS)) {
        (Some(g), Some(pinned)) => pinned.get(&g.to_lowercase()).copied().unwrap_or(0),
        (Some(_), None) => groups.iter().position(|g| *g == group).unwrap_or(0) as i32,
        (None, _) => 0,
    };
    // Fields the server fills in after loading are inherited filled in: when
    // nothing in the chain writes one, its default comes from the topmost
    // ancestor's values, not this block's.
    let root = root_of(store, k);
    let ra = if root == k {
        a.clone()
    } else {
        store.resolved(&root, "BlockType")
    };
    let root_draw = s_or(&ra, "DrawType").unwrap_or_default();
    let root_mat = s_or(&ra, "Material").unwrap_or_default();
    let root_group = ra.str_at("Group");
    let material: p::BlockMaterial = e(&a, "Material", p::BlockMaterial::from_name);
    let draw: p::DrawType = e(&a, "DrawType", p::DrawType::from_name);

    let support = match a.obj("Support").and_then(Value::as_object) {
        Some(m) => Some(by_face(m, |s| {
            Ok(p::RequiredBlockFaceSupport {
                face_type: s.str_at("FaceType"),
                self_face_type: s.str_at("SelfFaceType"),
                block_set_id: s.str_at("BlockSetId"),
                block_type_id: match s.str_at("BlockTypeId") {
                    Some(b) => ctx.index_or(PATH, BUILTIN, Some(&b), i32::MIN)?,
                    None => -1,
                },
                tag_index: s.str_at("TagId").map_or(i32::MIN, |t| ctx.tag(&t)),
                fluid_id: match s.str_at("FluidId") {
                    Some(f) => ctx.index_or(fluids::PATH, fluids::BUILTIN, Some(&f), i32::MIN)?,
                    None => -1,
                },
                support: support_match(s, "Support", "Required"),
                match_self: support_match(s, "MatchSelf", "Ignored"),
                allow_support_propagation: s.bool_or("AllowSupportPropagation", true),
                rotate: s.bool_or("Rotate", true),
                filler: fillers(s),
            })
        })?),
        None if root_mat == "Empty" && root_group.as_deref() != Some("@Tech") => Some(vec![(
            p::BlockNeighbor::DOWN,
            vec![p::RequiredBlockFaceSupport {
                face_type: Some("Full".into()),
                block_type_id: -1,
                tag_index: i32::MIN,
                fluid_id: -1,
                support: p::SupportMatch::from_name("Required").unwrap_or_default(),
                match_self: p::SupportMatch::from_name("Ignored").unwrap_or_default(),
                allow_support_propagation: true,
                rotate: true,
                ..Default::default()
            }],
        )]),
        None => None,
    };
    let supporting = match a.obj("Supporting").and_then(Value::as_object) {
        Some(m) => Some(by_face(m, |s| {
            Ok(p::BlockFaceSupport {
                face_type: Some(s.str_at("FaceType").unwrap_or_else(|| "Full".into())),
                filler: fillers(s),
            })
        })?),
        None if matches!(root_draw.as_str(), "Cube" | "CubeWithModel" | "GizmoCube")
            && root_mat == "Solid" =>
        {
            Some(
                // Every neighbor (all 26), fully.
                (0..26)
                    .map(|n| {
                        (
                            p::BlockNeighbor(n),
                            vec![p::BlockFaceSupport {
                                face_type: Some("Full".into()),
                                filler: None,
                            }],
                        )
                    })
                    .collect(),
            )
        }
        None => Some(Vec::new()),
    };

    let model_texture = match a
        .get("CustomModelTexture")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
    {
        Some(l) => {
            let total: i32 = l.iter().map(|t| t.i32_or("Weight", 0)).sum();
            l.iter()
                .map(|t| p::ModelTexture {
                    texture: t.str_at("Texture"),
                    weight: if total > 0 {
                        t.i32_or("Weight", 1) as f32 / total as f32
                    } else {
                        0.0
                    },
                })
                .collect()
        }
        None => vec![p::ModelTexture {
            texture: Some("BlockTextures/Unknown.png".into()),
            weight: 1.0,
        }],
    };

    // Tints: `Tint` sets every face and `TintUp`… one face; the faces
    // inherit, so the nearest asset in the chain that sets a face wins.
    let mut chain = vec![k.to_owned()];
    {
        let mut c = store.parents.get(k);
        while let Some(pk) = c {
            if chain.len() > 32 || !store.own.contains_key(pk) || chain.contains(pk) {
                break;
            }
            chain.push(pk.clone());
            c = store.parents.get(pk);
        }
    }
    let face = |all: &str, one: &str| -> Option<Value> {
        chain.iter().find_map(|x| {
            // Keys are read in file order, so the later of the per-face and
            // the all-faces key wins.
            let o = store.own.get(x)?.as_object()?;
            o.iter()
                .filter(|(k, v)| (*k == one || *k == all) && !v.is_null())
                .last()
                .map(|(_, v)| v.clone())
        })
    };
    let tint_of = |one: &str| -> i32 {
        face("Tint", one)
            .and_then(|v| {
                v.as_array()
                    .and_then(|l| l.first())
                    .and_then(Value::as_str)
                    .and_then(crate::values::parse_color)
            })
            .map_or(-1, |c| argb(&c))
    };
    let tint = p::Tint {
        top: tint_of("TintUp"),
        bottom: tint_of("TintDown"),
        front: tint_of("TintSouth"),
        back: tint_of("TintNorth"),
        left: tint_of("TintWest"),
        right: tint_of("TintEast"),
    };
    let bt = |one: &str| face("BiomeTint", one).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let biome_tint = p::Tint {
        top: bt("BiomeTintUp"),
        bottom: bt("BiomeTintDown"),
        front: bt("BiomeTintSouth"),
        back: bt("BiomeTintNorth"),
        left: bt("BiomeTintWest"),
        right: bt("BiomeTintEast"),
    };

    let hitbox = match s_or(&a, "HitboxType").as_deref() {
        Some("Full") | None => 0,
        Some(h) => Some(ctx.index_or(
            block_hitboxes::PATH,
            block_hitboxes::BUILTIN,
            Some(h),
            i32::MIN,
        )?)
        .filter(|i| *i != i32::MIN)
        .unwrap_or(0),
    };
    let interaction_hitbox = match a.str_at("InteractionHitboxType").as_deref() {
        None => i32::MIN,
        Some("Full") => 0,
        Some(h) => Some(ctx.index_or(
            block_hitboxes::PATH,
            block_hitboxes::BUILTIN,
            Some(h),
            i32::MIN,
        )?)
        .filter(|i| *i != i32::MIN)
        .unwrap_or(hitbox),
    };
    let sounds = s_or(&a, "BlockSoundSetId").unwrap_or_else(|| "EMPTY".into());
    let block_sound_set_index = if sounds == "EMPTY" {
        0
    } else {
        Some(ctx.index_or(
            sound_sets::BLOCK,
            sound_sets::EMPTY,
            Some(&sounds),
            i32::MIN,
        )?)
        .filter(|i| *i != i32::MIN)
        .unwrap_or(0)
    };
    let mat = s_or(&a, "PhysicalMaterialId").unwrap_or_else(|| "EMPTY".into());
    let physical_material_index = if mat == "EMPTY" {
        0
    } else {
        Some(ctx.index_or(
            materials::MATERIALS,
            materials::MATERIALS_BUILTIN,
            Some(&mat),
            i32::MIN,
        )?)
        .filter(|i| *i != i32::MIN)
        .unwrap_or(0)
    };

    let gathering = a.obj("Gathering").map(|g| {
        let owner = super::music::owner_of(store, k, "Gathering");
        let drop = |x: &Value, part: &str| {
            x.get("DropList")
                .and_then(|d| reference(d, &owner, &format!("Gathering_{part}_DropList")))
        };
        p::BlockGathering {
            breaking: g.obj("Breaking").map(|b| p::BlockBreaking {
                gather_type: b.str_at("GatherType"),
                health: 0.0,
                quantity: b.i32_or("Quantity", 1),
                quality: b.i32_or("Quality", 0),
                item_id: b.str_at("ItemId"),
                drop_list_id: drop(b, "Breaking"),
            }),
            harvest: g.obj("Harvest").map(|h| p::Harvesting {
                item_id: h.str_at("ItemId"),
                drop_list_id: drop(h, "Harvest"),
            }),
            soft: g.obj("Soft").map(|s| p::SoftBlock {
                item_id: s.str_at("ItemId"),
                drop_list_id: drop(s, "Soft"),
                is_weapon_breakable: s.bool_or("IsWeaponBreakable", true),
            }),
        }
    });
    let block_index = |key: Option<String>| -> Result<i32> {
        match key {
            Some(b) => ctx.index_or(PATH, BUILTIN, Some(&b), i32::MIN),
            None => Ok(-1),
        }
    };
    let placement_settings = match a.obj("PlacementSettings") {
        Some(s) => Some(p::BlockPlacementSettings {
            allow_rotation_key: s.bool_or("AllowRotationKey", true),
            place_in_empty_blocks: s.bool_or("PlaceInEmptyBlocks", false),
            preview_visibility: enum_or(
                s,
                "BlockPreviewVisibility",
                p::BlockPreviewVisibility::from_name,
                p::BlockPreviewVisibility::DEFAULT,
            ),
            rotation_mode: enum_or(
                s,
                "RotationMode",
                p::BlockPlacementRotationMode::from_name,
                p::BlockPlacementRotationMode::DEFAULT,
            ),
            wall_placement_override_block_id: block_index(
                s.str_at("WallPlacementOverrideBlockId"),
            )?,
            floor_placement_override_block_id: block_index(
                s.str_at("FloorPlacementOverrideBlockId"),
            )?,
            ceiling_placement_override_block_id: block_index(
                s.str_at("CeilingPlacementOverrideBlockId"),
            )?,
            allow_break_replace: s.bool_or("AllowBreakReplace", false),
        }),
        None => None,
    };

    // Interactions: combined by entry down the chain, inline roots keyed
    // after the asset that wrote them.
    let mut inter: Vec<(String, Value, String)> = Vec::new();
    {
        let mut chain = vec![k.to_owned()];
        let mut c = store.parents.get(k);
        while let Some(pk) = c {
            if chain.len() > 32 || !store.own.contains_key(pk) || chain.contains(pk) {
                break;
            }
            chain.push(pk.clone());
            c = store.parents.get(pk);
        }
        for owner in chain.iter().rev() {
            if let Some(Value::Object(m)) = store.own.get(owner).and_then(|o| o.get("Interactions"))
            {
                for (t, v) in m {
                    inter.retain(|(x, _, _)| x != t);
                    if !v.is_null() {
                        inter.push((t.clone(), v.clone(), owner.clone()));
                    }
                }
            }
        }
    }
    let bench = a.obj("Bench");
    let mut interactions = Vec::new();
    for (t, v, owner) in &inter {
        let Some(ty) = InteractionType::from_name(t) else {
            continue;
        };
        let key = reference(v, owner, &format!("Interactions_{t}"));
        interactions.push((
            ty,
            ctx.index_or(
                root_interactions::PATH,
                root_interactions::BUILTIN,
                key.as_deref(),
                i32::MIN,
            )?,
        ));
    }
    if let Some(b) = bench {
        if !inter.iter().any(|(t, _, _)| t == "Use") {
            let root = match b.str_at("Type").as_deref() {
                Some("Crafting") | None => Some("*Simple_Crafting_Default"),
                Some("DiagramCrafting") => Some("*Diagram_Crafting_Default"),
                Some("StructuralCrafting") => Some("*Structural_Crafting_Default"),
                _ => None,
            };
            if let Some(r) = root {
                interactions.push((
                    InteractionType::USE,
                    ctx.index_or(
                        root_interactions::PATH,
                        root_interactions::BUILTIN,
                        Some(r),
                        i32::MIN,
                    )?,
                ));
            }
        }
    }

    // States: own (or inherited) definitions, and `default` for state blocks.
    let states = match a.obj("State") {
        Some(st) => {
            let owner = super::music::owner_of(store, k, "State");
            let mut out = Vec::new();
            for name in st
                .obj("Definitions")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|m| m.keys())
            {
                let v = &st["Definitions"][name];
                if let Some(key) = reference(v, &owner, &format!("State_Definitions_{name}")) {
                    let i = ctx.index_or(PATH, BUILTIN, Some(&key), i32::MIN)?;
                    if i != i32::MIN {
                        out.push((name.clone(), i));
                    }
                }
            }
            if let Some(def) = container_block(store, k) {
                out.retain(|(n, _)| n != "default");
                out.push((
                    "default".into(),
                    ctx.index_or(PATH, BUILTIN, Some(&def), i32::MIN)?,
                ));
            }
            Some(out)
        }
        None => None,
    };

    let tag_indexes = {
        let idx: Vec<i32> = tag_names(store, items, k)
            .iter()
            .map(|n| ctx.tag(n))
            .collect();
        crate::chm::int_set_order(&idx, 0)
    };

    let interaction_hint = a.str_at("InteractionHint").or_else(|| {
        let a = &ra;
        Some(
            if a.obj("Bench").is_some() || a.bool_or("IsDoor", false) {
                "server.interactionHints.open"
            } else if a
                .obj("Gathering")
                .is_some_and(|g| g.get("Harvest").is_some())
            {
                "server.interactionHints.gather"
            } else if a
                .get("Seats")
                .is_some_and(|s| s.as_array().is_some_and(|x| !x.is_empty()) || s.is_object())
            {
                "server.interactionHints.sit"
            } else {
                "server.interactionHints.generic"
            }
            .to_owned(),
        )
    });

    Ok(p::BlockType {
        item: item_of(store, k),
        name: Some(k.to_owned()),
        unknown: false,
        draw_type: draw,
        material,
        opacity: e(&a, "Opacity", p::Opacity::from_name),
        shader_effect: Some(
            a.get("Effect")
                .and_then(Value::as_array)
                .filter(|l| !l.is_empty())
                .map(|l| {
                    l.iter()
                        .filter_map(|x| x.as_str().and_then(p::ShaderType::from_name))
                        .collect()
                })
                .unwrap_or_else(|| vec![p::ShaderType::NONE]),
        ),
        hitbox,
        interaction_hitbox,
        model: a.str_at("CustomModel"),
        model_texture: Some(model_texture),
        model_scale: f_or(&a, "CustomModelScale", 1.0),
        model_animation: a.str_at("CustomModelAnimation"),
        model_animation_speed: f_or(&a, "CustomModelAnimationSpeed", 1.0),
        looping: b_or(&a, "Looping", false),
        max_support_distance: field(&a, "MaxSupportDistance")
            .and_then(Value::as_i64)
            .unwrap_or(0) as i32,
        block_supports_required_for: e(
            &a,
            "SupportsRequiredFor",
            p::BlockSupportsRequiredForType::from_name,
        ),
        support,
        supporting,
        requires_alpha_blending: b_or(&a, "RequiresAlphaBlending", false),
        cube_textures: Some(
            fluids::block_textures(a.get("Textures")).unwrap_or_else(fluids::unknown_textures),
        ),
        cube_side_mask_texture: a.str_at("TextureSideMask"),
        cube_shading_mode: e(&a, "CubeShadingMode", p::ShadingMode::from_name),
        random_rotation: e(&a, "RandomRotation", p::RandomRotation::from_name),
        // The server's `Debug` rotation is sent as `UpDownNESW`.
        variant_rotation: match s_or(&a, "VariantRotation").as_deref() {
            Some("Debug") => p::VariantRotation::UP_DOWN_NESW,
            _ => e(&a, "VariantRotation", p::VariantRotation::from_name),
        },
        rotation_yaw_placement_offset: e(&a, "RotationYawPlacementOffset", p::Rotation::from_name),
        block_sound_set_index,
        physical_material_index,
        sound_occlusion_opacity: f_or(&a, "SoundOcclusionOpacity", 1.0),
        ambient_sound_event_index: sound_events::index(
            ctx,
            a.str_at("AmbientSoundEventId").as_deref(),
        )?,
        conditional_sounds: match a
            .get("ConditionalSounds")
            .and_then(Value::as_array)
            .filter(|l| !l.is_empty())
        {
            Some(l) => Some(
                l.iter()
                    .map(|c| {
                        Ok(p::ConditionalBlockSound {
                            sound_event_index: sound_events::index(
                                ctx,
                                c.str_at("SoundEventId").as_deref(),
                            )?,
                            ambience_fx_index: ctx.index_or(
                                super::ambience::PATH,
                                super::ambience::BUILTIN,
                                c.str_at("AmbienceFXId").as_deref(),
                                0,
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            None => None,
        },
        particles: a
            .get("Particles")
            .and_then(Value::as_array)
            .filter(|l| !l.is_empty())
            .map(|l| l.iter().map(model_particles::one).collect()),
        block_particle_set_id: a.str_at("BlockParticleSetId"),
        block_breaking_decal_id: a.str_at("BlockBreakingDecalId"),
        particle_color: a.color("ParticleColor"),
        texture_computed_color: a.color("TextureComputedColor"),
        light: a.obj("Light").map(fluids::light),
        tint: Some(tint),
        biome_tint: Some(biome_tint),
        group: group_id,
        transition_texture: a.str_at("TransitionTexture"),
        transition_to_groups: a
            .strings("TransitionToGroups")
            .filter(|g| !g.is_empty())
            .map(|l| {
                l.iter()
                    .map(|g| match ctx.pinned(GROUPS) {
                        Some(pinned) => pinned.get(&g.to_lowercase()).copied().unwrap_or(0),
                        None => groups
                            .iter()
                            .position(|x| x.as_deref() == Some(g.as_str()))
                            .unwrap_or(0) as i32,
                    })
                    .collect()
            }),
        movement_settings: {
            let m = a.obj("MovementSettings");
            let g = |k: &str, d: f32| m.map_or(d, |m| m.f32_or(k, d));
            let gb = |k: &str| m.is_some_and(|m| m.bool_or(k, false));
            p::BlockMovementSettings {
                is_climbable: gb("IsClimbable"),
                climb_up_speed_multiplier: g("ClimbUpSpeedMultiplier", 1.0),
                climb_down_speed_multiplier: g("ClimbDownSpeedMultiplier", 1.0),
                climb_lateral_speed_multiplier: g("ClimbLateralSpeedMultiplier", 1.0),
                is_bouncy: gb("IsBouncy"),
                bounce_velocity: g("BounceVelocity", 0.0),
                drag: g("Drag", 0.82),
                friction: g("Friction", 0.18),
                terminal_velocity_modifier: g("TerminalVelocityModifier", 1.0),
                horizontal_speed_multiplier: g("HorizontalSpeedMultiplier", 1.0),
                acceleration: 0.0,
                jump_force_multiplier: g("JumpForceMultiplier", 1.0),
                disable_auto_step: gb("DisableAutoStep"),
            }
        },
        interaction_hint,
        gathering,
        placement_settings,
        display: None,
        rail: a.obj("Rail").map(|r| p::RailConfig {
            points: r.get("Points").and_then(Value::as_array).map(|l| {
                l.iter()
                    .map(|pt| p::RailPoint {
                        point: pt.obj("Point").map(crate::values::vec3).unwrap_or_default(),
                        normal: pt
                            .obj("Normal")
                            .map(crate::values::vec3)
                            .unwrap_or_default(),
                    })
                    .collect()
            }),
        }),
        ignore_support_when_placed: b_or(&a, "IgnoreSupportWhenPlaced", false),
        interactions: Some(interactions),
        states,
        transition_to_tag: a
            .str_at("TransitionToTag")
            .map_or(i32::MIN, |t| ctx.tag(&t)),
        tag_indexes: Some(tag_indexes),
        bench: bench.map(|b| p::Bench {
            bench_tier_levels: b
                .get("TierLevels")
                .and_then(Value::as_array)
                .filter(|l| !l.is_empty())
                .map(|l| {
                    l.iter()
                        .map(|t| p::BenchTierLevel {
                            bench_upgrade_requirement: t.obj("UpgradeRequirement").map(|u| {
                                p::BenchUpgradeRequirement {
                                    material: u.get("Material").and_then(Value::as_array).map(
                                        |l| {
                                            l.iter()
                                                .map(|m| super::recipes::material(ctx, m))
                                                .collect()
                                        },
                                    ),
                                    time_seconds: u
                                        .get("TimeSeconds")
                                        .and_then(Value::as_f64)
                                        .unwrap_or(0.0),
                                }
                            }),
                            // Read as a float, sent as a double.
                            crafting_time_reduction_modifier: t
                                .get("CraftingTimeReductionModifier")
                                .and_then(Value::as_f64)
                                .map_or(0.0, |x| x as f32 as f64),
                            extra_input_slot: t.i32_or("ExtraInputSlot", 0),
                            extra_output_slot: t.i32_or("ExtraOutputSlot", 0),
                        })
                        .collect()
                }),
        }),
        connected_block_rule_set: match a.obj("ConnectedBlockRuleSet") {
            Some(r) => rule_set(ctx, store, k, r)?,
            None => None,
        },
    })
}

/// A connected block output (`{Block, State}`) resolved from block `base`:
/// the block (default `base`), then, with a `State`, that state of its base
/// block (`default` is the base block itself); -1 when it does not resolve.
fn output(ctx: &Ctx, store: &Store, base: &str, o: Option<&Value>) -> Result<i32> {
    let Some(o) = o.filter(|o| o.is_object()) else {
        return Ok(-1);
    };
    let mut key = o.str_at("Block").unwrap_or_else(|| base.to_owned());
    let Some(found) = store
        .own
        .keys()
        .find(|x| x.eq_ignore_ascii_case(&key))
        .cloned()
    else {
        return Ok(-1);
    };
    key = found;
    if let Some(state) = o.str_at("State") {
        let base_block = container_block(store, &key).unwrap_or(key.clone());
        if state == "default" {
            key = base_block;
        } else {
            let b = store.resolved(&base_block, "BlockType");
            let owner = super::music::owner_of(store, &base_block, "State");
            match b
                .pointer(&format!("/State/Definitions/{state}"))
                .and_then(|v| reference(v, &owner, &format!("State_Definitions_{state}")))
            {
                Some(k) => key = k,
                None => return Ok(-1),
            }
        }
    }
    let i = ctx.index_or(PATH, BUILTIN, Some(&key), i32::MIN)?;
    Ok(if i == i32::MIN { -1 } else { i })
}

fn stair(ctx: &Ctx, store: &Store, base: &str, r: &Value) -> Result<p::StairConnectedBlockRuleSet> {
    let own = ctx.index_or(PATH, BUILTIN, Some(base), i32::MIN)?;
    let one = |k: &str| -> Result<i32> {
        let i = output(ctx, store, base, r.get(k))?;
        Ok(if i == -1 { own } else { i })
    };
    Ok(p::StairConnectedBlockRuleSet {
        straight_block_id: one("Straight")?,
        corner_left_block_id: one("Corner_Left")?,
        corner_right_block_id: one("Corner_Right")?,
        inverted_corner_left_block_id: one("Inverted_Corner_Left")?,
        inverted_corner_right_block_id: one("Inverted_Corner_Right")?,
        material_name: Some(r.str_at("MaterialName").unwrap_or_else(|| "Stair".into())),
    })
}

/// `ConnectedBlockRuleSet` by `Type`: `Stair` (outputs for each stair shape,
/// the block itself when missing), `Roof` (`Regular` and `Hollow` stairs,
/// `Topper`, `Width` (1)), `Patterned` (`RuleSetId`, `TemplatedShapes` →
/// shape → block, unresolved ones left out). Others send nothing.
fn rule_set(
    ctx: &Ctx,
    store: &Store,
    k: &str,
    r: &Value,
) -> Result<Option<p::ConnectedBlockRuleSet>> {
    Ok(match r.str_at("Type").as_deref() {
        Some("Stair") => Some(p::ConnectedBlockRuleSet::StairConnectedBlockRuleSet(
            Box::new(stair(ctx, store, k, r)?),
        )),
        Some("Roof") => Some(p::ConnectedBlockRuleSet::RoofConnectedBlockRuleSet(
            Box::new(p::RoofConnectedBlockRuleSet {
                regular: match r.obj("Regular") {
                    Some(x) => Some(stair(ctx, store, k, x)?),
                    None => None,
                },
                hollow: match r.obj("Hollow") {
                    Some(x) => Some(stair(ctx, store, k, x)?),
                    None => None,
                },
                topper_block_id: match r.obj("Topper") {
                    Some(t) => {
                        let i = output(ctx, store, k, Some(t))?;
                        if i == -1 { i32::MIN } else { i }
                    }
                    None => -1,
                },
                width: r.i32_or("Width", 1),
                material_name: r.str_at("MaterialName"),
            }),
        )),
        Some("Patterned") => {
            let mut shapes = Vec::new();
            for (name, o) in r
                .obj("TemplatedShapes")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let i = output(ctx, store, k, Some(o))?;
                if i != -1 {
                    shapes.push((name.clone(), i));
                }
            }
            Some(p::ConnectedBlockRuleSet::PatternedConnectedBlockRuleSet(
                Box::new(p::PatternedConnectedBlockRuleSet {
                    rule_set_id: r.str_at("RuleSetId").unwrap_or_default(),
                    shape_name_to_block_id: Some(shapes),
                }),
            ))
        }
        _ => None,
    })
}

pub fn build(ctx: &Ctx) -> Result<p::UpdateBlockTypes> {
    let store = ctx.store(PATH)?;
    let items = ctx.store(ITEMS)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    // Group ids follow load order: file blocks without a parent by path,
    // then the others, then contained blocks in index order.
    // The built-in blocks come first: `Empty` in `@Tech`, then the others
    // without a group.
    let mut groups: Vec<Option<String>> = vec![Some("@Tech".into()), None];
    let mut load: Vec<&String> = order
        .iter()
        .filter(|k| store.files.contains_key(*k))
        .collect();
    load.sort_by_key(|k| (store.parents.contains_key(*k), store.files.get(*k)));
    load.extend(
        order
            .iter()
            .filter(|k| store.own.contains_key(*k) && !store.files.contains_key(*k)),
    );
    for k in load {
        let g = store.resolved(k, "BlockType").str_at("Group");
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    let mut list = Vec::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        let b = if store.own.contains_key(k) {
            block(ctx, &store, &items, k, &mut groups)?
        } else {
            let mut b = block_from(ctx, &store, &items, k, builtin_json(k), &mut groups)?;
            b.unknown = k == "Unknown";
            // Built-in blocks have no asset data, so no tag list.
            b.tag_indexes = None;
            b
        };
        list.push((i, b));
    }
    Ok(p::UpdateBlockTypes {
        r#type: super::INIT,
        max_id: super::max_id(&list).max(order.len() as i32),
        block_types: Some(list),
        update_block_textures: false,
        update_model_textures: false,
        update_models: false,
        update_map_geometry: false,
    })
}

/// A block's tags (spec 07, "Tags"): its container's raw tags (the item or
/// block it is written in, in that map's order), then its parent's (its
/// whole raw-tag map, in map order), then its own.
pub fn block_tags(store: &Store, items: &Store, k: &str) -> tags::Tags {
    fn go(store: &Store, items: &Store, k: &str, depth: usize) -> tags::Tags {
        let mut t = tags::Tags::default();
        if let Some((oc, ok, _)) = store.contained_files.get(k) {
            let container = match oc.as_str() {
                "Item" => Some(tags::of(items, ok)),
                "BlockType" if depth < 16 => Some(go(store, items, ok, depth + 1)),
                _ => None,
            };
            if let Some(c) = container {
                t.put(&c.raw_in_map_order());
            }
        }
        if let Some(p) = store
            .parents
            .get(k)
            .filter(|p| store.own.contains_key(*p) && depth < 16)
        {
            t.put(&go(store, items, p, depth + 1).raw_in_map_order());
        }
        if let Some(v) = store.own.get(k) {
            t.put(&tags::own_entries(v));
        }
        t
    }
    go(store, items, k, 0)
}

/// A block's expanded tag names in insertion order.
pub fn tag_names(store: &Store, items: &Store, k: &str) -> Vec<String> {
    block_tags(store, items, k).expanded
}

/// Compare runs: learns tag indexes from the captured block tag lists.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    use aerie_protocol::wire::Wire;
    let (cap, _) = <p::UpdateBlockTypes as Wire>::decode(captured, 0)?;
    let store = ctx.store(PATH)?;
    let items = ctx.store(ITEMS)?;
    let obs: Vec<(Vec<String>, Vec<i32>)> = cap
        .block_types
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, b)| {
            let k = b.name?;
            store.own.contains_key(&k).then(|| {
                (
                    tag_names(&store, &items, &k),
                    b.tag_indexes.unwrap_or_default(),
                )
            })
        })
        .collect();
    tags::learn(ctx, &obs);
    // Group ids, by group name.
    let mut pairs = Vec::new();
    for (_, b) in <p::UpdateBlockTypes as Wire>::decode(captured, 0)?
        .0
        .block_types
        .unwrap_or_default()
    {
        if let Some(k) = b.name.filter(|k| store.own.contains_key(k)) {
            let a = store.resolved(&k, "BlockType");
            if let Some(g) = a.str_at("Group") {
                pairs.push((g, b.group));
            }
            // Support `TagId`s appear in no tag list: each captured support
            // entry gives its tag's index directly.
            if let (Some(m), Some(sup)) = (a.obj("Support").and_then(Value::as_object), &b.support)
            {
                let ours = by_face(m, |s| Ok(s.str_at("TagId")))?;
                for (face, names) in ours {
                    let Some((_, cap)) = sup.iter().find(|(f, _)| *f == face) else {
                        continue;
                    };
                    for (n, c) in names.iter().zip(cap) {
                        if let Some(n) = n {
                            if c.tag_index != i32::MIN {
                                ctx.pin_tag(n, c.tag_index);
                            }
                        }
                    }
                }
            }
        }
    }
    ctx.pin(GROUPS, pairs);
    Ok(())
}
