//! Block support (spec 10): which blocks need support, whether they have
//! it, and what an unsupported block leaves behind.
//!
//! Rules come from the block registry Aerie sends (`UpdateBlockTypes`:
//! support, supporting faces, distances, tags), block sets and hitboxes;
//! drop types, physics drops and drop lists from the `physics` table
//! (aerie-assets).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use aerie_protocol::proto;
use aerie_protocol::wire::Wire;
use rand::Rng;
use serde_json::Value;

pub const TABLE: &str = "physics";
/// Support value of a block placed as decoration: never checked.
pub const DECO: u8 = 15;
/// Blocks exist from y 0 to 319.
pub const WORLD_HEIGHT: i32 = 320;

const UPDATE_BLOCK_TYPES: u32 = 40;
const UPDATE_BLOCK_HITBOXES: u32 = 41;
const UPDATE_BLOCK_SOUND_SETS: u32 = 42;
const UPDATE_BLOCK_SETS: u32 = 46;
const UPDATE_SOUND_EVENTS: u32 = 65;

/// The 26 faces in registry order: sides, edges, corners.
pub const DIRS: [[i32; 3]; 26] = [
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, -1],
    [1, 0, 0],
    [0, 0, 1],
    [-1, 0, 0],
    [0, 1, -1],
    [0, 1, 1],
    [1, 1, 0],
    [-1, 1, 0],
    [0, -1, -1],
    [0, -1, 1],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, -1],
    [1, 0, 1],
    [-1, 0, 1],
    [-1, 0, -1],
    [1, 1, -1],
    [1, 1, 1],
    [-1, 1, 1],
    [-1, 1, -1],
    [1, -1, -1],
    [1, -1, 1],
    [-1, -1, 1],
    [-1, -1, -1],
];

fn face_of(d: [i32; 3]) -> Option<usize> {
    DIRS.iter().position(|x| *x == d)
}

/// Quarter turns about Y (yaw), X (pitch) and Z (roll).
fn turn_y(v: [i32; 3], k: i32) -> [i32; 3] {
    (0..k.rem_euclid(4)).fold(v, |[x, y, z], _| [z, y, -x])
}
fn turn_x(v: [i32; 3], k: i32) -> [i32; 3] {
    (0..k.rem_euclid(4)).fold(v, |[x, y, z], _| [x, -z, y])
}
fn turn_z(v: [i32; 3], k: i32) -> [i32; 3] {
    (0..k.rem_euclid(4)).fold(v, |[x, y, z], _| [-y, x, z])
}

/// A vector turned by a rotation (yaw, pitch, roll quarter turns): roll
/// first, then pitch, then yaw.
pub fn rotate(v: [i32; 3], yaw: i32, pitch: i32, roll: i32) -> [i32; 3] {
    turn_y(turn_x(turn_z(v, roll), pitch), yaw)
}

/// Rotation index → (yaw, pitch, roll) quarter turns.
pub fn rotation(index: i32) -> (i32, i32, i32) {
    (index % 4, (index / 4) % 4, index / 16)
}

/// For each face, the faces of neighbours it meets and where those
/// neighbours are (offsets from the block).
fn connecting() -> Vec<Vec<(usize, [i32; 3])>> {
    (0..26)
        .map(|f| {
            let d = DIRS[f];
            let turned: Vec<[i32; 3]> = match f {
                0..=5 => vec![[-d[0], -d[1], -d[2]]],
                6 | 7 | 10 | 11 => (1..4).map(|k| rotate(d, k, 0, 0)).collect(),
                14..=17 => (1..4).map(|k| rotate(d, 0, k, 0)).collect(),
                8 | 9 | 12 | 13 => (1..4).map(|k| rotate(d, 0, 0, k)).collect(),
                _ => vec![
                    rotate(d, 1, 0, 0),
                    rotate(d, 2, 0, 0),
                    rotate(d, 0, 1, 0),
                    rotate(d, 0, 2, 0),
                    rotate(d, 0, 3, 0),
                    rotate(d, 0, 0, 2),
                    [-d[0], -d[1], -d[2]],
                ],
            };
            turned
                .into_iter()
                .filter_map(|c| {
                    let n = face_of(c)?;
                    let mut off = [0; 3];
                    for a in 0..3 {
                        if d[a] == -c[a] {
                            off[a] = d[a];
                        }
                    }
                    Some((n, off))
                })
                .collect()
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Req {
    face_type: Option<String>,
    self_face_type: Option<String>,
    block_set: Option<String>,
    block: i32,
    tag: i32,
    fluid: i32,
    /// 0 ignored, 1 required, 2 disallowed.
    support: i32,
    match_self: i32,
    propagate: bool,
    rotate: bool,
    filler: Option<Vec<[i32; 3]>>,
}

#[derive(Clone, Debug)]
pub struct Sup {
    face_type: String,
    filler: Option<Vec<[i32; 3]>>,
}

type Faces<T> = Vec<Vec<T>>;

/// A drop: item and/or drop list, and a quantity.
#[derive(Clone, Debug, Default)]
pub struct Drop {
    pub item: Option<String>,
    pub list: Option<String>,
    pub quantity: i32,
}

/// A block's item container (spec 21).
#[derive(Clone, Debug, Default)]
pub struct Container {
    pub capacity: usize,
    pub stay_open_when_empty: bool,
    pub open_sound: i32,
    pub close_sound: i32,
}

/// What a falling block does when it lands (spec 24).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Falling {
    /// 0 place, 1 break, 2 explode.
    pub impact: u8,
    pub drop_items: bool,
    /// The hitbox collision config its entity carries, -1 for none.
    pub collision: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Block {
    pub key: String,
    pub transparent: bool,
    pub solid: bool,
    pub unknown: bool,
    pub hitbox: i32,
    support: Faces<Req>,
    supporting: Faces<Sup>,
    all_full: bool,
    max_distance: i32,
    required_for_any: bool,
    tags: Vec<i32>,
    pub item: Option<String>,
    pub break_sound: i32,
    pub hit_sound: i32,
    pub harvest_sound: i32,
    /// 0 break, 1 destroy, 2 fall.
    pub drop_type: u8,
    /// `Gathering.Physics`, `Breaking`, `Soft`, `Harvest` in that order of use.
    pub physics_drop: Option<Drop>,
    pub breaking_drop: Option<Drop>,
    pub soft_drop: Option<Drop>,
    pub harvest_drop: Option<Drop>,
    pub deco_when_placed: bool,
    /// Blocks a client may place instead of this one (the placement
    /// settings' wall, floor and ceiling overrides).
    pub place_overrides: Vec<i32>,
    /// Interaction states: name → block id (`default` included).
    pub states: Vec<(String, i32)>,
    /// The sound of switching to an interaction state (on the default
    /// state's block): state name → sound.
    pub state_sounds: Vec<(String, i32)>,
    /// The rule set that fits this block to its neighbours (spec 23).
    pub connected: Option<proto::ConnectedBlockRuleSet>,
    /// A door (on its default state's block): whether it is horizontal.
    pub door: Option<bool>,
    /// Its falling block settings, when it has any.
    pub falling: Option<Falling>,
    /// The template that fits it to its neighbours and its shapes'
    /// blocks (spec 23), on the block that declares them.
    pub template: Option<crate::connected::Use>,
    /// How its yaw mirrors: 0 symmetric, 1 orthogonal, 2 orthogonal
    /// inverse.
    pub flip: u8,
    /// A block that holds items (on its default state's block).
    pub container: Option<Container>,
}

impl Block {
    pub fn has_rules(&self) -> bool {
        self.support.iter().any(|f| !f.is_empty()) || self.max_distance > 0
    }

    /// What breaking it by physics drops (spec 10, "Removing").
    pub fn physics_drops(&self) -> Drop {
        let d = self
            .physics_drop
            .clone()
            .map(|d| Drop { quantity: 1, ..d })
            .or_else(|| self.breaking_drop.clone())
            .or_else(|| self.soft_drop.clone().map(|d| Drop { quantity: 1, ..d }))
            .or_else(|| self.harvest_drop.clone().map(|d| Drop { quantity: 1, ..d }))
            .unwrap_or(Drop {
                quantity: 1,
                ..Default::default()
            });
        if d.item.is_none() && d.list.is_none() {
            Drop {
                item: self.item.clone(),
                ..d
            }
        } else {
            d
        }
    }
}

/// Everything block support needs about the world at one position.
pub trait World {
    /// Block id and rotation index.
    fn block(&self, x: i32, y: i32, z: i32) -> (i32, i32);
    fn support_value(&self, x: i32, y: i32, z: i32) -> u8;
}

/// What a check found (spec 10): no rule applies, directly supported,
/// supported at a distance, or unsupported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    NoRule,
    Direct,
    Distance(i32),
    Unsupported,
}

#[derive(Default)]
pub struct Rules {
    blocks: Vec<Option<Block>>,
    sets: HashMap<String, HashSet<i32>>,
    /// Hitbox index → its boxes (min, max).
    boxes: HashMap<i32, Vec<[f32; 6]>>,
    sound_distance: HashMap<i32, f32>,
    templates: HashMap<String, crate::connected::Template>,
    rule_sets: HashMap<String, proto::ConnectedBlockRuleSetAsset>,
    lists: HashMap<String, Value>,
    connecting: Vec<Vec<(usize, [i32; 3])>>,
    rotated: Mutex<HashMap<(i32, i32), (Faces<Req>, Faces<Sup>)>>,
}

fn v3(v: &proto::Vector3i) -> [i32; 3] {
    [v.x, v.y, v.z]
}

fn by_face<T, U>(
    list: &Option<Vec<(proto::BlockNeighbor, Vec<T>)>>,
    f: impl Fn(&T) -> U,
) -> Faces<U> {
    let mut out: Faces<U> = (0..26).map(|_| Vec::new()).collect();
    for (n, v) in list.iter().flatten() {
        if let Some(slot) = out.get_mut(n.0 as usize) {
            slot.extend(v.iter().map(&f));
        }
    }
    out
}

fn dash(s: &str) -> Option<String> {
    (s != "-").then(|| s.to_owned())
}

impl Rules {
    /// Whether block `id` is in the named block set.
    pub fn in_set(&self, set: &str, id: i32) -> bool {
        self.sets.get(set).is_some_and(|s| s.contains(&id))
    }

    pub fn new(registries: &[(u32, Vec<u8>)], table: &str) -> Self {
        let mut r = Rules {
            connecting: connecting(),
            ..Default::default()
        };
        let mut sound_sets: HashMap<i32, i32> = HashMap::new();
        let mut hit_sets: HashMap<i32, i32> = HashMap::new();
        let mut harvest_sets: HashMap<i32, i32> = HashMap::new();
        for (id, payload) in registries {
            match *id {
                UPDATE_BLOCK_SETS => {
                    if let Ok((m, _)) = proto::UpdateBlockSets::decode(payload, 0) {
                        for (name, s) in m.block_sets.into_iter().flatten() {
                            r.sets
                                .insert(name, s.blocks.unwrap_or_default().into_iter().collect());
                        }
                    }
                }
                UPDATE_BLOCK_HITBOXES => {
                    if let Ok((m, _)) = proto::UpdateBlockHitboxes::decode(payload, 0) {
                        for (i, hs) in m.block_base_hitboxes.into_iter().flatten() {
                            r.boxes.insert(
                                i,
                                hs.iter()
                                    .map(|h| [h.min_x, h.min_y, h.min_z, h.max_x, h.max_y, h.max_z])
                                    .collect(),
                            );
                        }
                    }
                }
                UPDATE_BLOCK_SOUND_SETS => {
                    if let Ok((m, _)) = proto::UpdateBlockSoundSets::decode(payload, 0) {
                        for (i, s) in m.block_sound_sets.into_iter().flatten() {
                            for (k, e) in s.sound_event_indices.iter().flatten() {
                                if *k == proto::BlockSoundEvent::BREAK {
                                    sound_sets.insert(i, *e);
                                }
                                if *k == proto::BlockSoundEvent::HIT {
                                    hit_sets.insert(i, *e);
                                }
                                if *k == proto::BlockSoundEvent::HARVEST {
                                    harvest_sets.insert(i, *e);
                                }
                            }
                        }
                    }
                }
                90 => {
                    // Patterned connected block rule sets (spec 23).
                    if let Ok((m, _)) = proto::UpdateConnectedBlockRuleSets::decode(payload, 0) {
                        for (k, a) in m.rule_sets.into_iter().flatten() {
                            r.rule_sets.insert(k, a);
                        }
                    }
                }
                UPDATE_SOUND_EVENTS => {
                    if let Ok((m, _)) = proto::UpdateSoundEvents::decode(payload, 0) {
                        for (i, s) in m.sound_events.into_iter().flatten() {
                            r.sound_distance.insert(i, s.max_distance);
                        }
                    }
                }
                _ => {}
            }
        }
        for (id, payload) in registries {
            if *id != UPDATE_BLOCK_TYPES {
                continue;
            }
            let Ok((m, _)) = proto::UpdateBlockTypes::decode(payload, 0) else {
                continue;
            };
            for (i, b) in m.block_types.into_iter().flatten() {
                let Ok(i) = usize::try_from(i) else { continue };
                if r.blocks.len() <= i {
                    r.blocks.resize(i + 1, None);
                }
                let support = by_face(&b.support, |s: &proto::RequiredBlockFaceSupport| Req {
                    face_type: s.face_type.clone(),
                    self_face_type: s.self_face_type.clone(),
                    block_set: s.block_set_id.clone(),
                    block: s.block_type_id,
                    tag: s.tag_index,
                    fluid: s.fluid_id,
                    support: s.support.0,
                    match_self: s.match_self.0,
                    propagate: s.allow_support_propagation,
                    rotate: s.rotate,
                    filler: s.filler.as_ref().map(|f| f.iter().map(v3).collect()),
                });
                let supporting = by_face(&b.supporting, |s: &proto::BlockFaceSupport| Sup {
                    face_type: s.face_type.clone().unwrap_or_else(|| "Full".into()),
                    filler: s.filler.as_ref().map(|f| f.iter().map(v3).collect()),
                });
                let all_full = supporting
                    .iter()
                    .all(|f| f.len() == 1 && f[0].face_type == "Full" && f[0].filler.is_none());
                let gathering = b.gathering.as_ref();
                r.blocks[i] = Some(Block {
                    key: b.name.clone().unwrap_or_default(),
                    transparent: b.opacity == proto::Opacity::TRANSPARENT,
                    solid: b.material == proto::BlockMaterial::SOLID,
                    states: b.states.clone().unwrap_or_default(),
                    connected: b.connected_block_rule_set.clone(),
                    unknown: b.unknown,
                    hitbox: b.hitbox,
                    support,
                    supporting,
                    all_full,
                    max_distance: b.max_support_distance,
                    required_for_any: b.block_supports_required_for
                        == proto::BlockSupportsRequiredForType::ANY,
                    tags: b.tag_indexes.clone().unwrap_or_default(),
                    item: b.item.clone(),
                    break_sound: sound_sets
                        .get(&b.block_sound_set_index)
                        .copied()
                        .unwrap_or(0),
                    hit_sound: hit_sets.get(&b.block_sound_set_index).copied().unwrap_or(0),
                    harvest_sound: harvest_sets
                        .get(&b.block_sound_set_index)
                        .copied()
                        .unwrap_or(0),
                    place_overrides: b
                        .placement_settings
                        .as_ref()
                        .map(|p| {
                            [
                                p.wall_placement_override_block_id,
                                p.floor_placement_override_block_id,
                                p.ceiling_placement_override_block_id,
                            ]
                            .into_iter()
                            .filter(|&i| i >= 0)
                            .collect()
                        })
                        .unwrap_or_default(),
                    breaking_drop: gathering.and_then(|g| g.breaking.as_ref()).map(|d| Drop {
                        item: d.item_id.clone(),
                        list: d.drop_list_id.clone(),
                        quantity: d.quantity,
                    }),
                    soft_drop: gathering.and_then(|g| g.soft.as_ref()).map(|d| Drop {
                        item: d.item_id.clone(),
                        list: d.drop_list_id.clone(),
                        quantity: 1,
                    }),
                    harvest_drop: gathering.and_then(|g| g.harvest.as_ref()).map(|d| Drop {
                        item: d.item_id.clone(),
                        list: d.drop_list_id.clone(),
                        quantity: 1,
                    }),
                    ..Default::default()
                });
            }
        }
        let by_key: HashMap<String, usize> = r
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| Some((b.as_ref()?.key.clone(), i)))
            .collect();
        for l in table.lines() {
            let (kind, rest) = l.split_once(' ').unwrap_or((l, ""));
            match kind {
                "P" => {
                    let w: Vec<&str> = rest.split_whitespace().collect();
                    let [key, drop, item, list, deco] = w[..] else {
                        continue;
                    };
                    let Some(b) = by_key.get(key).and_then(|&i| r.blocks[i].as_mut()) else {
                        continue;
                    };
                    b.drop_type = drop.parse().unwrap_or(0);
                    b.deco_when_placed = deco == "1";
                    if item != "-" || list != "-" {
                        b.physics_drop = Some(Drop {
                            item: if item == "!" { None } else { dash(item) },
                            list: dash(list),
                            quantity: 1,
                        });
                    }
                }
                "S" => {
                    let w: Vec<&str> = rest.split_whitespace().collect();
                    if let [key, state, sound] = w[..]
                        && let Some(b) = by_key.get(key).and_then(|&i| r.blocks[i].as_mut())
                    {
                        b.state_sounds
                            .push((state.to_owned(), sound.parse().unwrap_or(0)));
                    }
                }
                "CT" => {
                    if let Some((key, json)) = rest.split_once(' ')
                        && let Ok(v) = serde_json::from_str::<Value>(json)
                    {
                        r.templates
                            .insert(key.to_owned(), crate::connected::Template::from_json(&v));
                    }
                }
                "CB" => {
                    let w: Vec<&str> = rest.splitn(4, ' ').collect();
                    if let [key, owner, template, json] = w[..]
                        && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(json)
                        && let Some(&i) = by_key.get(key)
                    {
                        let shapes = map
                            .iter()
                            .map(|(shape, entries)| {
                                let list = entries
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|e| {
                                        let id = *by_key.get(e.get(0)?.as_str()?)?;
                                        Some((id as i32, e.get(1)?.as_i64()? as i32))
                                    })
                                    .collect();
                                (shape.clone(), list)
                            })
                            .collect();
                        if let Some(b) = r.blocks[i].as_mut() {
                            b.template = Some(crate::connected::Use {
                                owner: owner.to_owned(),
                                template: template.to_owned(),
                                shapes,
                            });
                        }
                    }
                }
                "CF" => {
                    let w: Vec<&str> = rest.split_whitespace().collect();
                    if let [key, flip] = w[..]
                        && let Some(b) = by_key.get(key).and_then(|&i| r.blocks[i].as_mut())
                    {
                        b.flip = flip.parse().unwrap_or(0);
                    }
                }
                "F" => {
                    let w: Vec<&str> = rest.split_whitespace().collect();
                    if let [key, impact, drops, collision] = w[..]
                        && let Some(b) = by_key.get(key).and_then(|&i| r.blocks[i].as_mut())
                    {
                        b.falling = Some(Falling {
                            impact: impact.parse().unwrap_or(0),
                            drop_items: drops == "1",
                            collision: collision.parse().unwrap_or(-1),
                        });
                    }
                }
                "D" => {
                    let w: Vec<&str> = rest.split_whitespace().collect();
                    if let [key, horizontal] = w[..]
                        && let Some(b) = by_key.get(key).and_then(|&i| r.blocks[i].as_mut())
                    {
                        b.door = Some(horizontal == "1");
                    }
                }
                "K" => {
                    let w: Vec<&str> = rest.split_whitespace().collect();
                    let [key, capacity, stay, open, close] = w[..] else {
                        continue;
                    };
                    if let Some(b) = by_key.get(key).and_then(|&i| r.blocks[i].as_mut()) {
                        b.container = Some(Container {
                            capacity: capacity.parse().unwrap_or(20),
                            stay_open_when_empty: stay == "1",
                            open_sound: open.parse().unwrap_or(0),
                            close_sound: close.parse().unwrap_or(0),
                        });
                    }
                }
                "L" => {
                    if let Some((key, json)) = rest.split_once(' ')
                        && let Ok(v) = serde_json::from_str(json)
                    {
                        r.lists.insert(key.to_owned(), v);
                    }
                }
                _ => {}
            }
        }
        r
    }

    /// The block of a block's default interaction state (itself without
    /// states).
    pub fn base(&self, id: i32) -> Option<&Block> {
        let b = self.block(id)?;
        Some(
            b.states
                .iter()
                .find(|(n, _)| n == "default")
                .and_then(|(_, i)| self.block(*i))
                .unwrap_or(b),
        )
    }

    /// A block's template and shape map, its own or its base block's.
    pub fn template_use(&self, id: i32) -> Option<&crate::connected::Use> {
        let b = self.block(id)?;
        b.template
            .as_ref()
            .or_else(|| self.base(id)?.template.as_ref())
    }

    pub fn rule_set(&self, id: &str) -> Option<&proto::ConnectedBlockRuleSetAsset> {
        self.rule_sets.get(id)
    }

    pub fn template(&self, name: &str) -> Option<&crate::connected::Template> {
        self.templates.get(name)
    }

    /// How a block's yaw mirrors (its own flip type or its base block's).
    pub fn flip(&self, id: i32) -> u8 {
        match self.block(id).map_or(0, |b| b.flip) {
            0 => self.base(id).map_or(0, |b| b.flip),
            f => f,
        }
    }

    /// The name of the interaction state a block is (`default` without).
    pub fn state_of(&self, id: i32) -> &str {
        self.block(id)
            .and_then(|b| b.states.iter().find(|(_, i)| *i == id))
            .map_or("default", |(n, _)| n.as_str())
    }

    /// What a block holds items with, through any of its interaction
    /// states.
    pub fn container(&self, id: i32) -> Option<(&Container, &Block)> {
        let b = self.block(id)?;
        let base = b
            .states
            .iter()
            .find(|(n, _)| n == "default")
            .and_then(|(_, i)| self.block(*i))
            .unwrap_or(b);
        base.container.as_ref().map(|c| (c, base))
    }

    pub fn len(&self) -> usize {
        self.blocks.iter().filter(|b| b.is_some()).count()
    }

    pub fn block(&self, id: i32) -> Option<&Block> {
        self.blocks.get(usize::try_from(id).ok()?)?.as_ref()
    }

    pub fn sound_distance(&self, index: i32) -> f32 {
        self.sound_distance.get(&index).copied().unwrap_or(0.0)
    }

    /// The block's rules for a rotation (requirements with `Rotate` turn,
    /// supporting faces turn unless the block supports fully).
    fn rotated(&self, id: i32, rot: i32) -> Option<(Faces<Req>, Faces<Sup>)> {
        let b = self.block(id)?;
        if rot == 0 {
            return Some((b.support.clone(), b.supporting.clone()));
        }
        if let Some(r) = self.rotated.lock().unwrap().get(&(id, rot)) {
            return Some(r.clone());
        }
        let (yaw, pitch, roll) = rotation(rot);
        let turn = |v: [i32; 3]| rotate(v, yaw, pitch, roll);
        let mut support: Faces<Req> = (0..26).map(|_| Vec::new()).collect();
        for (f, reqs) in b.support.iter().enumerate() {
            for q in reqs {
                if q.rotate {
                    let to = face_of(turn(DIRS[f])).unwrap_or(f);
                    let mut q = q.clone();
                    q.filler = q.filler.map(|v| v.into_iter().map(turn).collect());
                    support[to].push(q);
                } else {
                    support[f].push(q.clone());
                }
            }
        }
        let supporting = if b.all_full {
            b.supporting.clone()
        } else {
            let mut s: Faces<Sup> = (0..26).map(|_| Vec::new()).collect();
            for (f, sups) in b.supporting.iter().enumerate() {
                let to = face_of(turn(DIRS[f])).unwrap_or(f);
                for x in sups {
                    let mut x = x.clone();
                    x.filler = x.filler.map(|v| v.into_iter().map(turn).collect());
                    s[to].push(x);
                }
            }
            s
        };
        let r = (support, supporting);
        self.rotated.lock().unwrap().insert((id, rot), r.clone());
        Some(r)
    }

    /// Whether a block, turned so, has a supporting top face.
    pub fn supports_up(&self, id: i32, rot: i32) -> bool {
        self.rotated(id, rot).is_some_and(|r| !r.1[0].is_empty())
    }

    /// The block's bounding box for a rotation in doubles, as physics
    /// uses it (the unit box for a plain block). The registry keeps
    /// floats; their shortest decimal form gives back the asset's value.
    pub fn bounds_exact(&self, id: i32, rot: i32) -> [f64; 6] {
        let exact = |v: f32| v.to_string().parse::<f64>().unwrap_or(v as f64);
        let unit = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let Some(boxes) = self
            .block(id)
            .filter(|b| b.hitbox != 0)
            .and_then(|b| self.boxes.get(&b.hitbox))
        else {
            return unit;
        };
        if rot == 0 {
            let mut bb = [f64::MAX, f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MIN];
            for h in boxes {
                for a in 0..3 {
                    bb[a] = bb[a].min(exact(h[a]));
                    bb[a + 3] = bb[a + 3].max(exact(h[a + 3]));
                }
            }
            return bb;
        }
        match self.bounds(id, rot) {
            Some(b) => [
                exact(b[0]),
                exact(b[1]),
                exact(b[2]),
                exact(b[3]),
                exact(b[4]),
                exact(b[5]),
            ],
            None => unit,
        }
    }

    /// The block's hitbox for a rotation, as one bounding box; `None` for a
    /// plain block.
    fn bounds(&self, id: i32, rot: i32) -> Option<[f32; 6]> {
        let b = self.block(id)?;
        if b.hitbox == 0 {
            return None;
        }
        let boxes = self.boxes.get(&b.hitbox)?;
        let (yaw, pitch, roll) = rotation(rot);
        let mut bb = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
        for h in boxes {
            for corner in 0..8 {
                let p = [
                    if corner & 1 == 0 { h[0] } else { h[3] },
                    if corner & 2 == 0 { h[1] } else { h[4] },
                    if corner & 4 == 0 { h[2] } else { h[5] },
                ];
                let q = rotate_f([p[0] - 0.5, p[1] - 0.5, p[2] - 0.5], yaw, pitch, roll);
                for a in 0..3 {
                    bb[a] = bb[a].min(q[a] + 0.5);
                    bb[a + 3] = bb[a + 3].max(q[a] + 0.5);
                }
            }
        }
        Some(bb)
    }

    /// A solid block's hitbox boxes for a rotation (block-local), for
    /// collisions; `None` when it is not solid.
    pub fn solid_boxes(&self, id: i32, rot: i32) -> Option<Vec<[f64; 6]>> {
        let b = self.block(id)?;
        if !b.solid {
            return None;
        }
        let unit = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        if b.hitbox == 0 {
            return Some(vec![unit]);
        }
        let Some(boxes) = self.boxes.get(&b.hitbox) else {
            return Some(vec![unit]);
        };
        let (yaw, pitch, roll) = rotation(rot);
        Some(
            boxes
                .iter()
                .map(|h| {
                    let mut out = [f64::MAX, f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MIN];
                    for corner in 0..8 {
                        let p = [
                            if corner & 1 == 0 { h[0] } else { h[3] },
                            if corner & 2 == 0 { h[1] } else { h[4] },
                            if corner & 4 == 0 { h[2] } else { h[5] },
                        ];
                        let q = rotate_f([p[0] - 0.5, p[1] - 0.5, p[2] - 0.5], yaw, pitch, roll);
                        for a in 0..3 {
                            out[a] = out[a].min((q[a] + 0.5) as f64);
                            out[a + 3] = out[a + 3].max((q[a] + 0.5) as f64);
                        }
                    }
                    out
                })
                .collect(),
        )
    }

    /// The centre of a block's hitbox (block-local) for a rotation; `None`
    /// for a plain block.
    /// A block's front direction (x, z) for its rotation's yaw, and its
    /// unrotated hitbox's depth (processing benches throw items out of
    /// their front).
    pub fn front(&self, id: i32, rot: i32) -> ([f64; 2], Option<f64>) {
        let (yaw, _, _) = rotation(rot);
        let d = rotate_f([0.0, 0.0, 1.0], yaw, 0, 0);
        let depth = self.bounds(id, 0).map(|b| (b[5] - b[2]) as f64);
        ([d[0] as f64, d[2] as f64], depth)
    }

    pub fn center(&self, id: i32, rot: i32) -> Option<[f64; 3]> {
        let b = self.bounds(id, rot)?;
        Some([
            ((b[0] + b[3]) / 2.0) as f64,
            ((b[1] + b[4]) / 2.0) as f64,
            ((b[2] + b[5]) / 2.0) as f64,
        ])
    }

    /// Cells a block's hitbox covers, relative to its main cell (rounded
    /// outward); just the main cell for a plain block.
    pub fn cells(&self, id: i32, rot: i32) -> Vec<[i32; 3]> {
        match self.bounds(id, rot) {
            Some(b) => {
                let lo = [
                    b[0].floor() as i32,
                    b[1].floor() as i32,
                    b[2].floor() as i32,
                ];
                let hi = [b[3].ceil() as i32, b[4].ceil() as i32, b[5].ceil() as i32];
                let mut v = Vec::new();
                for x in lo[0]..hi[0].max(lo[0] + 1) {
                    for y in lo[1]..hi[1].max(lo[1] + 1) {
                        for z in lo[2]..hi[2].max(lo[2] + 1) {
                            v.push([x, y, z]);
                        }
                    }
                }
                v
            }
            None => vec![[0, 0, 0]],
        }
    }

    fn protrudes(&self, id: i32, rot: i32) -> bool {
        self.bounds(id, rot).is_some_and(|b| {
            b[0] < 0.0 || b[1] < 0.0 || b[2] < 0.0 || b[3] > 1.0 || b[4] > 1.0 || b[5] > 1.0
        })
    }

    /// Whether `cell` (relative to the main cell) is inside the block.
    fn covers(&self, id: i32, rot: i32, cell: [i32; 3]) -> bool {
        match self.bounds(id, rot) {
            Some(b) => {
                (0..3).all(|a| cell[a] >= b[a].floor() as i32 && cell[a] < b[a + 3].ceil() as i32)
            }
            None => cell == [0, 0, 0],
        }
    }

    /// Checks a block's support (spec 10, "Checking one block").
    pub fn check(&self, w: &impl World, x: i32, y: i32, z: i32) -> Found {
        let (id, rot) = w.block(x, y, z);
        let Some(b) = self.block(id) else {
            return Found::NoRule;
        };
        if id == 0 || !b.has_rules() {
            return Found::NoRule;
        }
        if !self.protrudes(id, rot) {
            return self.check_cell(w, [x, y, z], id, rot, [0, 0, 0]);
        }
        let mut best = Found::NoRule;
        for c in self.cells(id, rot) {
            let at = [x + c[0], y + c[1], z + c[2]];
            let cell_rot = if c == [0, 0, 0] {
                rot
            } else {
                w.block(at[0], at[1], at[2]).1
            };
            let f = self.check_cell(w, at, id, cell_rot, c);
            if f == Found::NoRule {
                continue;
            }
            if b.required_for_any {
                match f {
                    Found::Direct => return Found::Direct,
                    Found::Unsupported => best = Found::Unsupported,
                    Found::Distance(d) => {
                        if dist_code(best) < d {
                            best = Found::Distance(d)
                        }
                    }
                    Found::NoRule => {}
                }
            } else {
                match f {
                    Found::Unsupported => return Found::Unsupported,
                    Found::Direct => best = Found::Direct,
                    Found::Distance(d) => {
                        if best == Found::NoRule {
                            best = Found::Distance(d)
                        }
                    }
                    Found::NoRule => {}
                }
            }
        }
        best
    }

    fn check_cell(
        &self,
        w: &impl World,
        at: [i32; 3],
        id: i32,
        rot: i32,
        filler: [i32; 3],
    ) -> Found {
        let Some(b) = self.block(id) else {
            return Found::NoRule;
        };
        if b.unknown {
            return Found::NoRule;
        }
        let Some((support, supporting)) = self.rotated(id, rot) else {
            return Found::NoRule;
        };
        if support.iter().all(|f| f.is_empty()) {
            return Found::NoRule;
        }
        let origin = [at[0] - filler[0], at[1] - filler[1], at[2] - filler[2]];
        let mut tested = false;
        let mut lowest = i32::MAX;
        for (face, reqs) in support.iter().enumerate() {
            if reqs.is_empty() {
                continue;
            }
            for &(nface, off) in &self.connecting[face] {
                let n = [at[0] + off[0], at[1] + off[1], at[2] + off[2]];
                let rel = [n[0] - origin[0], n[1] - origin[1], n[2] - origin[2]];
                if self.covers(id, rot, rel) {
                    continue;
                }
                let (nid, nrot) = w.block(n[0], n[1], n[2]);
                let Some(nb) = self.block(nid) else { continue };
                let mut satisfied = false;
                let mut failed = false;
                for q in reqs {
                    if q.filler.as_ref().is_some_and(|f| !f.contains(&filler)) {
                        continue;
                    }
                    let ok = self.satisfies(q, id, b, &supporting, face, nid, nb, nrot, nface);
                    if ok && b.max_distance > 0 && q.propagate {
                        let d = w.support_value(n[0], n[1], n[2]) as i32;
                        if d == DECO as i32 {
                            lowest = 1;
                        } else if d < lowest {
                            lowest = d;
                        }
                    }
                    match q.support {
                        1 => {
                            if ok {
                                satisfied = true;
                            }
                            tested = true;
                        }
                        2 => {
                            if ok {
                                failed = true;
                            }
                            tested = true;
                        }
                        _ => {}
                    }
                }
                if !failed && satisfied {
                    return Found::Direct;
                }
            }
        }
        if !tested {
            return Found::NoRule;
        }
        if lowest < i32::MAX && lowest >= 0 && b.max_distance >= lowest + 1 {
            return Found::Distance(lowest + 1);
        }
        Found::Unsupported
    }

    #[allow(clippy::too_many_arguments)]
    fn satisfies(
        &self,
        q: &Req,
        id: i32,
        _b: &Block,
        own_supporting: &Faces<Sup>,
        face: usize,
        nid: i32,
        nb: &Block,
        nrot: i32,
        nface: usize,
    ) -> bool {
        let mut ok = true;
        if let Some(set) = &q.block_set
            && !self.sets.get(set).is_some_and(|s| s.contains(&nid))
        {
            ok = false;
        }
        if ok && q.block >= 0 && q.block != nid {
            ok = false;
        }
        // No fluids in Aerie's world yet: a fluid requirement never holds.
        if ok && q.fluid >= 0 {
            ok = false;
        }
        if q.tag >= 0 && !nb.tags.contains(&q.tag) {
            ok = false;
        }
        if ok && let Some(ft) = &q.face_type {
            let sup = match self.rotated(nid, nrot) {
                Some((_, s)) => s,
                None => return false,
            };
            ok = offers(&sup[nface], ft, [0, 0, 0]);
        }
        if ok && let Some(ft) = &q.self_face_type {
            // Looked up with the neighbour's rotation, as the official
            // server does.
            let own = if nrot == 0 {
                own_supporting.clone()
            } else {
                match self.rotated(id, nrot) {
                    Some((_, s)) => s,
                    None => return false,
                }
            };
            ok = offers(&own[face], ft, [0, 0, 0]);
        }
        match q.match_self {
            1 => ok && nid == id,
            2 => ok && nid != id,
            _ => ok,
        }
    }

    /// Items a drop gives (spec 10, "Drop lists"), `quantity` times for a
    /// list.
    pub fn roll(&self, d: &Drop) -> Vec<(String, i32)> {
        let mut out = Vec::new();
        let mut rng = rand::thread_rng();
        if let Some(list) = &d.list {
            for _ in 0..d.quantity.max(0) {
                if let Some(c) = self.lists.get(list) {
                    let mut seen = HashSet::from([list.clone()]);
                    let mut picked = Vec::new();
                    self.populate(c, &mut rng, &mut seen, &mut picked);
                    for (item, min, max) in picked {
                        let n = rng.gen_range(0..(max - min + 1).max(1)) + min;
                        if n > 0 {
                            out.push((item, n));
                        }
                    }
                }
            }
        }
        if let Some(item) = &d.item {
            out.push((item.clone(), d.quantity));
        }
        out
    }

    fn populate(
        &self,
        c: &Value,
        rng: &mut impl Rng,
        seen: &mut HashSet<String>,
        out: &mut Vec<(String, i32, i32)>,
    ) {
        let int = |k: &str, d: i64| c.get(k).and_then(Value::as_i64).unwrap_or(d) as i32;
        let weight = |x: &Value| x.get("Weight").and_then(Value::as_f64).unwrap_or(100.0);
        let children: Vec<&Value> = c
            .get("Containers")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        match c.get("Type").and_then(Value::as_str).unwrap_or("Multiple") {
            "Multiple" => {
                let (min, max) = (int("MinCount", 1), int("MaxCount", 1));
                let r: f64 = rng.r#gen();
                let count = (r * (max - min) as f64 + min as f64 + 0.5).floor() as i32;
                for _ in 0..count {
                    for ch in &children {
                        let r: f64 = rng.r#gen();
                        if weight(ch) >= r * 100.0 {
                            self.populate(ch, rng, seen, out);
                        }
                    }
                }
            }
            "Choice" => {
                let (min, max) = (int("RollsMin", 1), int("RollsMax", 1));
                let r: f64 = rng.r#gen();
                let count = min + (r * (max - min + 1) as f64) as i32;
                let total: f64 = children.iter().map(|ch| weight(ch)).sum();
                for _ in 0..count {
                    let r: f64 = rng.r#gen();
                    let mut left = r.min(0.99999) * total;
                    for ch in &children {
                        left -= weight(ch);
                        if left <= 9.99999999995449E-6 {
                            self.populate(ch, rng, seen, out);
                            break;
                        }
                    }
                }
            }
            "Single" => {
                if let Some(i) = c.get("Item")
                    && let Some(id) = i.get("ItemId").and_then(Value::as_str)
                {
                    let q = |k: &str| i.get(k).and_then(Value::as_i64).unwrap_or(1) as i32;
                    out.push((id.to_owned(), q("QuantityMin"), q("QuantityMax")));
                }
            }
            "Droplist" => {
                if let Some(id) = c.get("DroplistId").and_then(Value::as_str)
                    && seen.insert(id.to_owned())
                    && let Some(l) = self.lists.get(id)
                {
                    self.populate(l, rng, seen, out);
                }
            }
            _ => {}
        }
    }
}

fn dist_code(f: Found) -> i32 {
    match f {
        Found::Distance(d) => d,
        Found::Unsupported => 0,
        _ => -1,
    }
}

fn offers(sups: &[Sup], face_type: &str, filler: [i32; 3]) -> bool {
    sups.iter()
        .any(|s| s.face_type == face_type && s.filler.as_ref().is_none_or(|f| f.contains(&filler)))
}

fn rotate_f(v: [f32; 3], yaw: i32, pitch: i32, roll: i32) -> [f32; 3] {
    let mut p = v;
    for _ in 0..roll.rem_euclid(4) {
        p = [-p[1], p[0], p[2]];
    }
    for _ in 0..pitch.rem_euclid(4) {
        p = [p[0], -p[2], p[1]];
    }
    for _ in 0..yaw.rem_euclid(4) {
        p = [p[2], p[1], -p[0]];
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_meet_their_opposites() {
        let c = connecting();
        // Up meets the block above's down face.
        assert_eq!(c[0], vec![(1, [0, 1, 0])]);
        // North-east edge: three neighbours' edges.
        assert_eq!(c[14].len(), 3);
        // A corner: seven.
        assert_eq!(c[18].len(), 7);
    }

    #[test]
    fn rotations() {
        assert_eq!(rotate([0, 0, -1], 1, 0, 0), [-1, 0, 0]);
        assert_eq!(rotation(16 + 4 + 3), (3, 1, 1));
    }

    struct Column {
        blocks: HashMap<[i32; 3], i32>,
        values: HashMap<[i32; 3], u8>,
    }
    impl World for Column {
        fn block(&self, x: i32, y: i32, z: i32) -> (i32, i32) {
            (*self.blocks.get(&[x, y, z]).unwrap_or(&0), 0)
        }
        fn support_value(&self, x: i32, y: i32, z: i32) -> u8 {
            *self.values.get(&[x, y, z]).unwrap_or(&0)
        }
    }

    fn rules() -> Rules {
        let full = |ft: &str| Sup {
            face_type: ft.into(),
            filler: None,
        };
        let req = |ft: Option<&str>, tag: i32, support: i32, propagate: bool| Req {
            face_type: ft.map(Into::into),
            self_face_type: None,
            block_set: None,
            block: -1,
            tag,
            fluid: -1,
            support,
            match_self: 0,
            propagate,
            rotate: true,
            filler: None,
        };
        let empty = || (0..26).map(|_| Vec::new()).collect::<Faces<Req>>();
        let mut stone = Block {
            key: "Stone".into(),
            supporting: (0..26).map(|_| vec![full("Full")]).collect(),
            all_full: true,
            support: empty(),
            ..Default::default()
        };
        stone.support = empty();
        let mut trunk = stone.clone();
        trunk.key = "Trunk".into();
        trunk.tags = vec![7];
        trunk.max_distance = 5;
        trunk.support[1] = vec![req(Some("Full"), i32::MIN, 1, false)];
        for f in [2, 3, 4, 5, 14, 15, 16, 17] {
            trunk.support[f] = vec![req(None, 7, 0, true)];
        }
        let mut leaves = Block {
            key: "Leaves".into(),
            tags: vec![8],
            max_distance: 4,
            support: empty(),
            supporting: (0..26).map(|_| Vec::new()).collect(),
            ..Default::default()
        };
        for f in 0..6 {
            leaves.support[f] = vec![req(None, 8, 0, true), req(None, 7, 1, true)];
        }
        Rules {
            blocks: vec![
                Some(Block {
                    key: "Empty".into(),
                    support: empty(),
                    supporting: (0..26).map(|_| Vec::new()).collect(),
                    ..Default::default()
                }),
                Some(stone),
                Some(trunk),
                Some(leaves),
            ],
            connecting: connecting(),
            ..Default::default()
        }
    }

    #[test]
    fn trunk_and_leaves() {
        let r = rules();
        let mut w = Column {
            blocks: HashMap::from([
                ([0, 0, 0], 1),
                ([0, 1, 0], 2),
                ([0, 2, 0], 2),
                ([1, 2, 0], 3),
                ([2, 2, 0], 3),
            ]),
            values: HashMap::new(),
        };
        assert_eq!(r.check(&w, 0, 0, 0), Found::NoRule);
        assert_eq!(r.check(&w, 0, 1, 0), Found::Direct);
        assert_eq!(r.check(&w, 0, 2, 0), Found::Direct);
        assert_eq!(r.check(&w, 1, 2, 0), Found::Direct);
        // Leaves next to leaves only: the neighbour's value plus one.
        assert_eq!(r.check(&w, 2, 2, 0), Found::Distance(1));
        w.blocks.remove(&[0, 0, 0]);
        assert_eq!(r.check(&w, 0, 1, 0), Found::Unsupported);
        w.blocks.remove(&[0, 2, 0]);
        w.values.insert([2, 2, 0], 4);
        assert_eq!(r.check(&w, 1, 2, 0), Found::Unsupported);
    }
}
