//! What the probe knows, built only from what the server sends it: the
//! registries it needs to act by name (items, blocks, root interactions,
//! stats, effects), the blocks around it, and its player (position, look,
//! game mode, inventory, active slots, stats, effects, windows, chat).
//! `report` turns it into JSON, the same shape for every server, so the
//! reports of two servers can be diffed.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aerie_protocol::wire::Wire;
use aerie_protocol::{Frame, proto};
use serde_json::{Map, Value, json};
use tracing::warn;

use crate::chunk;

/// Packet ids the probe reads or sends.
pub mod id {
    pub const DISCONNECT: u32 = 2;
    pub const PING: u32 = 3;
    pub const NETWORK_TICK: u32 = 5;
    pub const BLOCK_TYPES: u32 = 40;
    pub const ENTITY_EFFECTS: u32 = 51;
    pub const ITEMS: u32 = 54;
    pub const ROOT_INTERACTIONS: u32 = 67;
    pub const UNARMED_INTERACTIONS: u32 = 68;
    pub const STAT_TYPES: u32 = 72;
    pub const SET_CLIENT_ID: u32 = 100;
    pub const SET_GAME_MODE: u32 = 101;
    pub const JOIN_WORLD: u32 = 104;
    pub const CLIENT_MOVEMENT: u32 = 108;
    pub const CLIENT_TELEPORT: u32 = 109;
    pub const SET_CHUNK: u32 = 131;
    pub const SET_BLOCK: u32 = 140;
    pub const SET_BLOCKS: u32 = 141;
    pub const ENTITY_UPDATES: u32 = 161;
    pub const PLAYER_INVENTORY: u32 = 170;
    pub const MOVE_ITEM_STACK: u32 = 175;
    pub const SET_ACTIVE_SLOT: u32 = 177;
    pub const INVENTORY_ACTION: u32 = 179;
    pub const OPEN_WINDOW: u32 = 200;
    pub const UPDATE_WINDOW: u32 = 201;
    pub const CLOSE_WINDOW: u32 = 202;
    pub const SERVER_MESSAGE: u32 = 210;
    pub const CHAT_MESSAGE: u32 = 211;
    pub const INTERACTION_CHAINS: u32 = 290;
}

/// The inventory sections a player has, by the ids clients use.
pub const SECTIONS: [(&str, i32); 8] = [
    ("hotbar", -1),
    ("storage", -2),
    ("armor", -3),
    ("utility", -5),
    ("tools", -8),
    ("backpack", -9),
    ("abilities", -11),
    ("rune_bag", -12),
];

pub fn section_name(id: i32) -> String {
    SECTIONS
        .iter()
        .find(|s| s.1 == id)
        .map_or_else(|| format!("window{id}"), |s| s.0.to_owned())
}

/// A section id from its name or number.
pub fn section_id(s: &str) -> Option<i32> {
    SECTIONS
        .iter()
        .find(|x| x.0 == s)
        .map(|x| x.1)
        .or_else(|| s.strip_prefix("window").unwrap_or(s).parse().ok())
}

pub struct BlockInfo {
    pub name: String,
    pub interactions: Vec<(i32, i32)>,
}

pub struct StatInfo {
    pub name: String,
    pub value: f32,
    pub min: f32,
    pub max: f32,
}

#[derive(Default)]
pub struct Registries {
    /// Item id → its interactions (interaction type, root interaction).
    pub items: HashMap<String, Vec<(i32, i32)>>,
    pub blocks: HashMap<i32, BlockInfo>,
    pub roots: BTreeMap<i32, String>,
    /// What an empty hand does.
    pub unarmed: Vec<(i32, i32)>,
    pub stats: HashMap<i32, StatInfo>,
    pub effects: HashMap<i32, String>,
}

impl Registries {
    pub fn block_name(&self, id: i32) -> String {
        self.blocks
            .get(&id)
            .map_or_else(|| format!("#{id}"), |b| b.name.clone())
    }

    pub fn root_name(&self, id: i32) -> String {
        self.roots
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("#{id}"))
    }

    pub fn root_by_name(&self, name: &str) -> Option<i32> {
        self.roots.iter().find(|r| r.1 == name).map(|r| *r.0)
    }

    /// The root interaction an item (or an empty hand) runs for a type.
    pub fn item_root(&self, item: Option<&str>, kind: i32) -> Option<i32> {
        let list = match item {
            Some(i) => self.items.get(i)?,
            None => &self.unarmed,
        };
        list.iter().find(|x| x.0 == kind).map(|x| x.1)
    }
}

/// One section of blocks, decoded only when something looks inside.
enum Sec {
    Raw(Vec<u8>),
    Blocks(Vec<i32>),
    Air,
    Unreadable,
}

/// The blocks the server sent: whole sections, then single changes.
#[derive(Default)]
pub struct World {
    sections: HashMap<(i32, i32, i32), Sec>,
}

fn split(v: i32) -> (i32, usize) {
    (v.div_euclid(32), v.rem_euclid(32) as usize)
}

impl World {
    fn blocks(&mut self, key: (i32, i32, i32)) -> Option<&mut Vec<i32>> {
        let s = self.sections.get_mut(&key)?;
        if let Sec::Raw(d) = s {
            *s = match chunk::Section::decode(d) {
                Ok(x) => Sec::Blocks(x.blocks),
                Err(e) => {
                    warn!("section {key:?} unreadable: {e}");
                    Sec::Unreadable
                }
            };
        }
        if let Sec::Air = s {
            *s = Sec::Blocks(vec![0; chunk::VOLUME]);
        }
        match s {
            Sec::Blocks(b) => Some(b),
            _ => None,
        }
    }

    /// The block at a position, if its section was sent.
    pub fn get(&mut self, x: i32, y: i32, z: i32) -> Option<i32> {
        let ((sx, lx), (sy, ly), (sz, lz)) = (split(x), split(y), split(z));
        if let Some(Sec::Air) = self.sections.get(&(sx, sy, sz)) {
            return Some(0);
        }
        self.blocks((sx, sy, sz))
            .map(|b| b[chunk::index(lx, ly, lz)])
    }

    fn set(&mut self, sec: (i32, i32, i32), index: usize, block: i32) {
        if let Some(b) = self.blocks(sec)
            && let Some(v) = b.get_mut(index)
        {
            *v = block;
        }
    }

    pub fn len(&self) -> usize {
        self.sections.len()
    }
}

#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub quantity: i32,
    pub durability: f64,
    pub max_durability: f64,
}

#[derive(Clone, Default)]
pub struct Section {
    pub capacity: i16,
    pub slots: BTreeMap<i32, Item>,
}

impl Section {
    fn from(s: &proto::InventorySection) -> Self {
        Section {
            capacity: s.capacity,
            slots: s
                .items
                .iter()
                .flatten()
                .map(|(i, it)| {
                    (
                        *i,
                        Item {
                            id: it.item_id.clone(),
                            quantity: it.quantity,
                            durability: it.durability,
                            max_durability: it.max_durability,
                        },
                    )
                })
                .collect(),
        }
    }

    fn report(&self) -> Value {
        let slots: Map<String, Value> = self
            .slots
            .iter()
            .map(|(i, it)| {
                let mut v = json!({"item": it.id, "quantity": it.quantity});
                if it.max_durability > 0.0 {
                    v["durability"] = json!(it.durability);
                    v["max_durability"] = json!(it.max_durability);
                }
                (i.to_string(), v)
            })
            .collect();
        json!({"capacity": self.capacity, "slots": slots})
    }
}

pub struct Window {
    pub kind: i32,
    pub data: Option<String>,
    pub section: Option<Section>,
}

pub struct Effect {
    pub name: String,
    pub remaining: f32,
    pub infinite: bool,
    pub debuff: bool,
}

/// Something a script can wait for.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Packet(u32),
    Chat(String),
    WindowOpened(i32),
    WindowClosed(i32),
    Block([i32; 3]),
    Teleport,
    Inventory,
}

#[derive(Default)]
pub struct State {
    pub reg: Registries,
    pub world: World,
    pub client_id: Option<i32>,
    pub joined: bool,
    pub position: Option<[f64; 3]>,
    pub body: Option<[f32; 3]>,
    pub look: Option<[f32; 3]>,
    pub game_mode: Option<i32>,
    pub sections: BTreeMap<i32, Section>,
    pub active: BTreeMap<i32, i32>,
    pub stats: BTreeMap<i32, f32>,
    pub modifiers: BTreeMap<i32, BTreeSet<String>>,
    pub effects: BTreeMap<i32, Effect>,
    pub windows: BTreeMap<i32, Window>,
    pub chat: Vec<String>,
    pub teleports: u32,
    pub counts: BTreeMap<u32, u32>,
    pub disconnect: Option<String>,
    /// Every event so far; a wait looks at the ones after a mark.
    pub events: Vec<Event>,
    /// The next interaction chain id the probe uses.
    pub next_chain: i32,
}

/// The text of a message: its raw text, or its key and parameters.
pub fn message_text(m: &proto::FormattedMessage) -> String {
    let mut s = m
        .raw_text
        .clone()
        .or_else(|| m.message_id.clone())
        .unwrap_or_default();
    for (k, v) in m.params.iter().flatten() {
        let v = match v {
            proto::ParamValue::StringParamValue(v) => v.value.clone().unwrap_or_default(),
            proto::ParamValue::BoolParamValue(v) => v.value.to_string(),
            proto::ParamValue::DoubleParamValue(v) => v.value.to_string(),
            proto::ParamValue::IntParamValue(v) => v.value.to_string(),
            proto::ParamValue::LongParamValue(v) => v.value.to_string(),
        };
        s += &format!(" {k}={v}");
    }
    for (k, v) in m.message_params.iter().flatten() {
        s += &format!(" {k}=[{}]", message_text(v));
    }
    for c in m.children.iter().flatten() {
        s += &message_text(c);
    }
    s
}

fn dir(d: &proto::Direction) -> [f32; 3] {
    [d.yaw, d.pitch, d.roll]
}

impl State {
    pub fn new() -> Self {
        State {
            next_chain: 1,
            ..Default::default()
        }
    }

    /// The item in the active hotbar slot.
    pub fn held(&self) -> Option<&Item> {
        let slot = self.active.get(&-1).copied().unwrap_or(0);
        self.sections.get(&-1)?.slots.get(&slot)
    }

    /// Takes in one packet from the server. Decoding errors are logged and
    /// the packet skipped: the probe must keep going on a server whose
    /// packets it reads wrong.
    pub fn observe(&mut self, f: &Frame) {
        *self.counts.entry(f.id).or_default() += 1;
        if let Err(e) = self.observe_inner(f) {
            warn!("state: packet {} not understood: {e}", f.id);
        }
        if !matches!(f.id, id::PING | id::NETWORK_TICK) {
            self.events.push(Event::Packet(f.id));
        }
    }

    fn observe_inner(&mut self, f: &Frame) -> anyhow::Result<()> {
        let p = &f.payload;
        match f.id {
            id::ITEMS => {
                let (m, _) = proto::UpdateItems::decode(p, 0)?;
                for (name, it) in m.items.into_iter().flatten() {
                    let list = it
                        .interactions
                        .into_iter()
                        .flatten()
                        .map(|(t, r)| (t.0, r))
                        .collect();
                    self.reg.items.insert(name, list);
                }
            }
            id::BLOCK_TYPES => {
                let (m, _) = proto::UpdateBlockTypes::decode(p, 0)?;
                for (i, b) in m.block_types.into_iter().flatten() {
                    let name = b.name.or(b.item).unwrap_or_else(|| format!("#{i}"));
                    let interactions = b
                        .interactions
                        .into_iter()
                        .flatten()
                        .map(|(t, r)| (t.0, r))
                        .collect();
                    self.reg.blocks.insert(i, BlockInfo { name, interactions });
                }
            }
            id::ROOT_INTERACTIONS => {
                let (m, _) = proto::UpdateRootInteractions::decode(p, 0)?;
                for (i, r) in m.interactions.into_iter().flatten() {
                    self.reg
                        .roots
                        .insert(i, r.id.unwrap_or_else(|| format!("#{i}")));
                }
            }
            id::UNARMED_INTERACTIONS => {
                let (m, _) = proto::UpdateUnarmedInteractions::decode(p, 0)?;
                self.reg.unarmed = m
                    .interactions
                    .into_iter()
                    .flatten()
                    .map(|(t, r)| (t.0, r))
                    .collect();
            }
            id::STAT_TYPES => {
                let (m, _) = proto::UpdateEntityStatTypes::decode(p, 0)?;
                for (i, t) in m.types.into_iter().flatten() {
                    self.reg.stats.insert(
                        i,
                        StatInfo {
                            name: t.id.unwrap_or_else(|| format!("#{i}")),
                            value: t.value,
                            min: t.min,
                            max: t.max,
                        },
                    );
                }
            }
            id::ENTITY_EFFECTS => {
                let (m, _) = proto::UpdateEntityEffects::decode(p, 0)?;
                for (i, e) in m.entity_effects.into_iter().flatten() {
                    self.reg
                        .effects
                        .insert(i, e.id.or(e.name).unwrap_or_else(|| format!("#{i}")));
                }
            }
            id::SET_CLIENT_ID => {
                self.client_id = Some(proto::SetClientId::decode(p, 0)?.0.client_id);
            }
            id::JOIN_WORLD => self.joined = true,
            id::SET_GAME_MODE => {
                self.game_mode = Some(proto::SetGameMode::decode(p, 0)?.0.game_mode.0);
            }
            id::CLIENT_TELEPORT => {
                let (t, _) = proto::ClientTeleport::decode(p, 0)?;
                self.transform(&t.model_transform);
                self.teleports += 1;
                self.events.push(Event::Teleport);
            }
            id::SET_CHUNK => {
                let (c, _) = proto::SetChunk::decode(p, 0)?;
                let s = match c.data {
                    Some(d) => Sec::Raw(d.into_iter().map(|b| b as u8).collect()),
                    None => Sec::Air,
                };
                self.world.sections.insert((c.x, c.y, c.z), s);
            }
            id::SET_BLOCK => {
                let (b, _) = proto::ServerSetBlock::decode(p, 0)?;
                let ((sx, lx), (sy, ly), (sz, lz)) = (split(b.x), split(b.y), split(b.z));
                self.world
                    .set((sx, sy, sz), chunk::index(lx, ly, lz), b.block_id);
                self.events.push(Event::Block([b.x, b.y, b.z]));
            }
            id::SET_BLOCKS => {
                let (b, _) = proto::ServerSetBlocks::decode(p, 0)?;
                for c in &b.cmds {
                    let i = c.index as u16 as usize;
                    self.world.set((b.x, b.y, b.z), i, c.block_id);
                    let (x, z, y) = (i & 31, (i >> 5) & 31, i >> 10);
                    self.events.push(Event::Block([
                        b.x * 32 + x as i32,
                        b.y * 32 + y as i32,
                        b.z * 32 + z as i32,
                    ]));
                }
            }
            id::ENTITY_UPDATES => {
                let (u, _) = proto::EntityUpdates::decode(p, 0)?;
                for e in u.updates.into_iter().flatten() {
                    if Some(e.network_id) != self.client_id {
                        continue;
                    }
                    for c in e.updates.into_iter().flatten() {
                        self.component(c);
                    }
                }
            }
            id::PLAYER_INVENTORY => {
                let (m, _) = proto::UpdatePlayerInventory::decode(p, 0)?;
                let all = [
                    (-2, &m.storage),
                    (-3, &m.armor),
                    (-1, &m.hotbar),
                    (-5, &m.utility),
                    (-8, &m.tools),
                    (-9, &m.backpack),
                    (-11, &m.ability_slots),
                    (-12, &m.rune_bag),
                ];
                for (i, s) in all {
                    if let Some(s) = s {
                        self.sections.insert(i, Section::from(s));
                    }
                }
                self.events.push(Event::Inventory);
            }
            id::SET_ACTIVE_SLOT => {
                let (m, _) = proto::SetActiveSlot::decode(p, 0)?;
                self.active.insert(m.inventory_section_id, m.active_slot);
            }
            id::OPEN_WINDOW => {
                let (m, _) = proto::OpenWindow::decode(p, 0)?;
                self.windows.insert(
                    m.id,
                    Window {
                        kind: m.window_type.0,
                        data: m.window_data,
                        section: m.inventory.as_ref().map(Section::from),
                    },
                );
                self.events.push(Event::WindowOpened(m.id));
            }
            id::UPDATE_WINDOW => {
                let (m, _) = proto::UpdateWindow::decode(p, 0)?;
                if let Some(w) = self.windows.get_mut(&m.id) {
                    if m.window_data.is_some() {
                        w.data = m.window_data;
                    }
                    if let Some(s) = &m.inventory {
                        w.section = Some(Section::from(s));
                    }
                }
            }
            id::CLOSE_WINDOW => {
                let (m, _) = proto::CloseWindow::decode(p, 0)?;
                self.windows.remove(&m.id);
                self.events.push(Event::WindowClosed(m.id));
            }
            id::SERVER_MESSAGE => {
                let (m, _) = proto::ServerMessage::decode(p, 0)?;
                let text = m.message.as_ref().map(message_text).unwrap_or_default();
                self.chat.push(text.clone());
                self.events.push(Event::Chat(text));
            }
            id::DISCONNECT => {
                let (m, _) = proto::ServerDisconnect::decode(p, 0)?;
                self.disconnect = Some(m.reason.as_ref().map(message_text).unwrap_or_default());
            }
            _ => {}
        }
        Ok(())
    }

    fn transform(&mut self, t: &proto::ModelTransform) {
        if let Some(p) = &t.position {
            self.position = Some([p.x, p.y, p.z]);
        }
        if let Some(d) = &t.body_orientation {
            self.body = Some(dir(d));
        }
        if let Some(d) = &t.look_orientation {
            self.look = Some(dir(d));
        }
    }

    fn component(&mut self, c: proto::ComponentUpdate) {
        match c {
            proto::ComponentUpdate::TransformUpdate(t) => self.transform(&t.transform),
            proto::ComponentUpdate::EntityStatsUpdate(s) => {
                for (stat, ops) in s.entity_stat_updates {
                    for op in ops {
                        self.stat(stat, &op);
                    }
                }
            }
            proto::ComponentUpdate::EntityEffectsUpdate(e) => {
                for u in e.entity_effect_updates {
                    if u.r#type == proto::EffectOp::REMOVE {
                        self.effects.remove(&u.id);
                    } else {
                        let name = self
                            .reg
                            .effects
                            .get(&u.id)
                            .cloned()
                            .unwrap_or_else(|| format!("#{}", u.id));
                        self.effects.insert(
                            u.id,
                            Effect {
                                name,
                                remaining: u.remaining_time,
                                infinite: u.infinite,
                                debuff: u.debuff,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn stat(&mut self, stat: i32, u: &proto::EntityStatUpdate) {
        let info = self.reg.stats.get(&stat);
        let (lo, hi, base) = info.map_or((f32::MIN, f32::MAX, 0.0), |i| (i.min, i.max, i.value));
        let v = self.stats.entry(stat).or_insert(base);
        match u.op {
            proto::EntityStatOp::INIT | proto::EntityStatOp::SET => *v = u.value,
            proto::EntityStatOp::ADD => *v += u.value,
            proto::EntityStatOp::MINIMIZE => *v = lo,
            proto::EntityStatOp::MAXIMIZE => *v = hi,
            proto::EntityStatOp::RESET => *v = base,
            proto::EntityStatOp::REMOVE => {
                self.stats.remove(&stat);
                self.modifiers.remove(&stat);
            }
            proto::EntityStatOp::PUT_MODIFIER => {
                if let Some(k) = &u.modifier_key {
                    self.modifiers.entry(stat).or_default().insert(k.clone());
                }
            }
            proto::EntityStatOp::REMOVE_MODIFIER => {
                if let Some(k) = &u.modifier_key {
                    self.modifiers.entry(stat).or_default().remove(k);
                }
            }
            _ => {}
        }
    }

    fn stat_name(&self, i: i32) -> String {
        self.reg
            .stats
            .get(&i)
            .map_or_else(|| format!("#{i}"), |s| s.name.clone())
    }

    /// The whole state as JSON. Ids that differ between servers (blocks,
    /// effects, stats, roots) are written by name.
    pub fn report(&self) -> Value {
        let round = |v: f64| (v * 1000.0).round() / 1000.0;
        let sections: Map<String, Value> = self
            .sections
            .iter()
            .map(|(i, s)| (section_name(*i), s.report()))
            .collect();
        let active: Map<String, Value> = self
            .active
            .iter()
            .map(|(i, s)| (section_name(*i), json!(s)))
            .collect();
        let stats: Map<String, Value> = self
            .stats
            .iter()
            .map(|(i, v)| {
                let mut s = json!({"value": v});
                if let Some(m) = self.modifiers.get(i).filter(|m| !m.is_empty()) {
                    s["modifiers"] = json!(m);
                }
                (self.stat_name(*i), s)
            })
            .collect();
        let effects: Vec<Value> = self
            .effects
            .values()
            .map(|e| {
                json!({"effect": e.name, "remaining": e.remaining, "infinite": e.infinite, "debuff": e.debuff})
            })
            .collect();
        let windows: Map<String, Value> = self
            .windows
            .iter()
            .map(|(i, w)| {
                let data = w
                    .data
                    .as_deref()
                    .map(|d| serde_json::from_str(d).unwrap_or_else(|_| json!(d)));
                (
                    i.to_string(),
                    json!({"type": w.kind, "data": data, "inventory": w.section.as_ref().map(Section::report)}),
                )
            })
            .collect();
        let counts: Map<String, Value> = self
            .counts
            .iter()
            .map(|(i, n)| {
                let name = aerie_protocol::table::info(*i).map_or("?", |x| x.name);
                (format!("{i} {name}"), json!(n))
            })
            .collect();
        json!({
            "client_id": self.client_id,
            "joined": self.joined,
            "position": self.position.map(|p| p.map(round)),
            "body": self.body,
            "look": self.look,
            "game_mode": self.game_mode.map(|m| match m { 0 => "adventure".to_owned(), 1 => "creative".to_owned(), m => m.to_string() }),
            "held": self.held().map(|i| i.id.clone()),
            "active_slots": active,
            "inventory": sections,
            "stats": stats,
            "effects": effects,
            "windows": windows,
            "chat": self.chat,
            "teleports": self.teleports,
            "disconnect": self.disconnect,
            "sections_known": self.world.len(),
            "packets": counts,
        })
    }
}
