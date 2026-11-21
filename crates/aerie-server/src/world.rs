//! The players in the world: who is here, where they are, and what they
//! say. Every connection registers with the [`Hub`]; packets for a player go
//! through its outbox and are written by that player's connection task.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use aerie_protocol::proto;
use aerie_protocol::wire::Wire;
use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::inventory::{self, Inventory};

/// Packet ids used here (gen/packets.tsv).
pub mod id {
    pub const SET_GAME_MODE: u32 = 101;
    pub const CLIENT_MOVEMENT: u32 = 108;
    pub const CLIENT_TELEPORT: u32 = 109;
    pub const SERVER_MESSAGE: u32 = 210;
    pub const SERVER_SET_BLOCK: u32 = 140;
    pub const CHAT_MESSAGE: u32 = 211;
    pub const SYNC_INTERACTION_CHAINS: u32 = 290;
    pub const ENTITY_UPDATES: u32 = 161;
    pub const UPDATE_BLOCK_DAMAGE: u32 = 144;
    pub const DROP_CREATIVE_ITEM: u32 = 172;
    pub const DROP_ITEM_STACK: u32 = 174;
    pub const MOVE_ITEM_STACK: u32 = 175;
    pub const SMART_MOVE_ITEM_STACK: u32 = 176;
    pub const SET_CREATIVE_ITEM: u32 = 171;
    pub const SMART_GIVE_CREATIVE_ITEM: u32 = 173;
    pub const UPDATE_PLAYER_INVENTORY: u32 = 170;
    pub const PLAY_SOUND_2D: u32 = 154;
    pub const PLAY_SOUND_3D: u32 = 155;
    pub const OPEN_WINDOW: u32 = 200;
    pub const UPDATE_WINDOW: u32 = 201;
    pub const CLOSE_WINDOW: u32 = 202;
    pub const SEND_WINDOW_ACTION: u32 = 203;
    pub const CLIENT_OPEN_WINDOW: u32 = 204;
    pub const SET_PAGE: u32 = 216;
    pub const ADD_TO_PLAYER_LIST: u32 = 224;
    pub const REMOVE_FROM_PLAYER_LIST: u32 = 225;
}

/// An item lying on the ground.
#[derive(Clone)]
struct Dropped {
    item: String,
    quantity: i32,
    at: proto::Position,
    since: std::time::Instant,
}

/// Network ids of dropped items start here, far from players'.
const FIRST_ITEM_ID: i32 = 1_000_000;
/// How close a player picks an item up, and after how long.
const PICKUP_RADIUS: f64 = 1.75;
const PICKUP_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// A block change: position and block id (0 is air).
pub type BlockChange = ((i32, i32, i32), i32);

/// Hotbar slots.

/// Blocks a player gets on joining, those that exist.
const STARTER: &[&str] = &[
    "Rock_Stone",
    "Soil_Dirt",
    "Soil_Grass",
    "Rock_Stone_Brick",
    "Wood_Oak_Planks",
    "Wood_Oak_Trunk",
    "Rock_Sandstone",
    "Soil_Sand",
    "Glass",
];

/// Most blocks `/fill` changes at once.
const FILL_MAX: i64 = 32 * 32 * 32;

/// A serialized packet; its channel picks the stream.
pub type Packet = (u32, Vec<u8>);

pub fn packet<T: Wire>(id: u32, p: &T) -> Result<Packet> {
    let mut payload = Vec::new();
    p.encode(&mut payload)?;
    Ok((id, payload))
}

struct Entry {
    name: String,
    uuid: uuid::Uuid,
    outbox: mpsc::UnboundedSender<Packet>,
    position: Option<proto::Position>,
    /// Id of the last `ClientTeleport` sent; the client acknowledges it in
    /// `ClientMovement`.
    teleport_id: i8,
    /// What it carries.
    inv: Inventory,
    /// Creative mode (the built world's default).
    creative: bool,
    /// How other players see this one (set on entering the world); its
    /// transform is kept current.
    view: Option<proto::EntityUpdate>,
    body: Option<proto::Direction>,
    look: Option<proto::Direction>,
    /// The open crafting window.
    window: Option<crate::crafting::Window>,
    /// Ids for windows the server opens start at 1.
    next_window: i32,
    /// Chains that may use up the held item: chain id → (item, hotbar slot).
    consuming: HashMap<i32, (String, i32)>,
}

#[derive(Default)]
pub struct Hub {
    inner: Mutex<Inner>,
    /// Lower-cased block key → block id, from the BlockTypes registry.
    blocks: HashMap<String, i32>,
    /// Lower-cased block key → the key as written (item ids are exact).
    names: HashMap<String, String>,
    /// Where changed blocks are saved (`--world DIR`), if anywhere.
    save: Option<std::path::PathBuf>,
    /// Block damage and drops.
    gather: crate::gather::Table,
    /// Block id → key as written.
    keys_by_id: HashMap<i32, String>,
    /// Recipes, materials and benches.
    book: crate::crafting::Book,
    /// Items used up by an interaction: (item, interaction type) → (charge
    /// time, quantity change, Adventure only).
    consume: HashMap<(String, i32), (f32, i32, bool)>,
    /// The ground under the changes: the generator, or flat layers.
    ground: Mutex<Option<(Vec<i32>, Option<crate::terrain::Generator>)>>,
}

#[derive(Default)]
struct Inner {
    next_id: i32,
    players: BTreeMap<i32, Entry>,
    /// Blocks changed since the server started, sent to whoever joins.
    placed: BTreeMap<(i32, i32, i32), i32>,
    /// Damaged blocks: health left (1 is whole).
    health: HashMap<(i32, i32, i32), f32>,
    /// Items lying on the ground, by network id.
    items: BTreeMap<i32, Dropped>,
    next_item: i32,
}

impl Hub {
    #[cfg(test)]
    pub fn new(blocks: HashMap<String, i32>) -> Self {
        Self::with_names(blocks, Vec::new())
    }

    pub fn with_names(blocks: HashMap<String, i32>, names: Vec<String>) -> Self {
        let names: HashMap<String, String> =
            names.into_iter().map(|n| (n.to_lowercase(), n)).collect();
        let keys_by_id = blocks
            .iter()
            .map(|(k, &i)| (i, names.get(k).cloned().unwrap_or_else(|| k.clone())))
            .collect();
        Self {
            blocks,
            names,
            keys_by_id,
            ..Default::default()
        }
    }

    pub fn with_book(mut self, b: crate::crafting::Book) -> Self {
        self.book = b;
        self
    }

    /// The `consume` table (aerie-assets): `F item type charge adjust adventure`.
    pub fn with_consume(mut self, text: &str) -> Self {
        for l in text.lines() {
            if let ["F", item, ty, charge, adjust, adv] =
                l.split_whitespace().collect::<Vec<_>>()[..]
                && let (Ok(ty), Ok(charge), Ok(adjust)) =
                    (ty.parse(), charge.parse(), adjust.parse())
            {
                self.consume
                    .insert((item.to_owned(), ty), (charge, adjust, adv == "1"));
            }
        }
        self
    }

    /// Follows chains that can use up the held item (eating): once the
    /// client reports a charge reaching the item's `ModifyInventory`, the
    /// held stack changes as the official server's interaction does.
    fn consume_step(&self, player: i32, c: &proto::SyncInteractionChain) -> Result<()> {
        let tracked = {
            let mut g = self.inner.lock().unwrap();
            let Some(e) = g.players.get_mut(&player) else {
                return Ok(());
            };
            if c.initial
                && let Some(item) = &c.item_in_hand_id
                && self
                    .consume
                    .contains_key(&(item.clone(), c.interaction_type.0))
            {
                e.consuming
                    .insert(c.chain_id, (item.clone(), c.active_hotbar_slot));
            }
            let t = e.consuming.get(&c.chain_id).cloned();
            if c.state != proto::InteractionState::NOT_FINISHED {
                e.consuming.remove(&c.chain_id);
            }
            t.map(|t| (t, e.creative))
        };
        let Some(((item, slot), creative)) = tracked else {
            return Ok(());
        };
        let Some(&(charge, adjust, adventure)) =
            self.consume.get(&(item.clone(), c.interaction_type.0))
        else {
            return Ok(());
        };
        let reached = c
            .interaction_data
            .iter()
            .flatten()
            .flatten()
            .any(|d| d.charge_value >= charge && d.charge_value >= 0.0);
        if !reached || (adventure && creative) {
            return Ok(());
        }
        if let Some(e) = self.inner.lock().unwrap().players.get_mut(&player) {
            e.consuming.remove(&c.chain_id);
        }
        self.with_inventory(player, |inv| {
            if let Some((held, n)) = inv.get(inventory::HOTBAR, slot)
                && held == item
            {
                inv.set(inventory::HOTBAR, slot, Some((held, n + adjust)));
            }
        })?;
        info!("{player} used {item} ({adjust:+})");
        Ok(())
    }

    pub fn with_gather(mut self, t: crate::gather::Table) -> Self {
        self.gather = t;
        self
    }

    /// The ground the world is made of (set once, by the first join).
    pub fn set_ground(&self, layers: Vec<i32>, generator: Option<crate::terrain::Generator>) {
        let mut g = self.ground.lock().unwrap();
        if g.is_none() {
            *g = Some((layers, generator));
        }
    }

    /// The block at a position: changed, else generated.
    pub fn block_in_world(&self, x: i32, y: i32, z: i32) -> i32 {
        if let Some(b) = self.block_at(x, y, z) {
            return b;
        }
        match &*self.ground.lock().unwrap() {
            Some((_, Some(g))) => g.block(x, y, z),
            Some((layers, None)) => usize::try_from(y)
                .ok()
                .and_then(|y| layers.get(y))
                .copied()
                .unwrap_or(0),
            None => 0,
        }
    }

    /// Whether a player plays in creative (set by /gamemode).
    pub fn set_creative(&self, id: i32, creative: bool) {
        if let Some(e) = self.inner.lock().unwrap().players.get_mut(&id) {
            e.creative = creative;
        }
    }

    /// One hit on a block: creative breaks it at once; otherwise the hit
    /// takes the held tool's power (or the bare hand's) from the block's
    /// health, everyone sees the cracks (`UpdateBlockDamage`), and at no
    /// health it breaks and drops its item. Returns the change when it
    /// breaks.
    fn hit(
        &self,
        player: i32,
        x: i32,
        y: i32,
        z: i32,
        held: Option<&str>,
    ) -> Result<Option<BlockChange>> {
        let creative = self
            .inner
            .lock()
            .unwrap()
            .players
            .get(&player)
            .is_none_or(|e| e.creative);
        if creative || self.gather.is_empty() {
            return Ok(Some(((x, y, z), 0)));
        }
        let block = self.block_in_world(x, y, z);
        if block == 0 {
            return Ok(None);
        }
        let Some(key) = self.keys_by_id.get(&block).cloned() else {
            return Ok(None);
        };
        let held = held.filter(|h| !h.is_empty() && *h != "Empty");
        let Some((power, drop)) = self.gather.hit(&key, held) else {
            return Ok(None);
        };
        let left = {
            let mut g = self.inner.lock().unwrap();
            let h = g.health.entry((x, y, z)).or_insert(1.0);
            *h -= power;
            let left = *h;
            if left <= 0.0 {
                g.health.remove(&(x, y, z));
            }
            left
        };
        self.broadcast(&packet(
            id::UPDATE_BLOCK_DAMAGE,
            &proto::UpdateBlockDamage {
                block_position: Some(proto::BlockPosition { x, y, z }),
                damage: left.max(0.0),
                delta: -power,
            },
        )?);
        if left > 0.0 {
            return Ok(None);
        }
        if let Some((item, n)) = drop {
            self.drop_item(&item, n, x as f64 + 0.5, y as f64 + 0.25, z as f64 + 0.5)?;
        }
        Ok(Some(((x, y, z), 0)))
    }

    /// An item on the ground, seen by everyone.
    pub fn drop_item(&self, item: &str, quantity: i32, x: f64, y: f64, z: f64) -> Result<()> {
        let (nid, d) = {
            let mut g = self.inner.lock().unwrap();
            g.next_item = g.next_item.max(FIRST_ITEM_ID) + 1;
            let nid = g.next_item;
            let d = Dropped {
                item: item.to_owned(),
                quantity,
                at: proto::Position { x, y, z },
                since: std::time::Instant::now(),
            };
            g.items.insert(nid, d.clone());
            (nid, d)
        };
        self.broadcast(&packet(
            id::ENTITY_UPDATES,
            &proto::EntityUpdates {
                removed: None,
                updates: Some(vec![item_entity(nid, &d)]),
            },
        )?);
        Ok(())
    }

    /// Items near a player go into its inventory.
    pub fn pick_up(&self, player: i32) -> Result<()> {
        let taken: Vec<(i32, Dropped)> = {
            let mut g = self.inner.lock().unwrap();
            let Some(p) = g.players.get(&player).and_then(|e| e.position.clone()) else {
                return Ok(());
            };
            let near: Vec<i32> = g
                .items
                .iter()
                .filter(|(_, d)| {
                    d.since.elapsed() >= PICKUP_DELAY
                        && (d.at.x - p.x).powi(2)
                            + (d.at.y - (p.y + 0.5)).powi(2)
                            + (d.at.z - p.z).powi(2)
                            <= PICKUP_RADIUS * PICKUP_RADIUS
                })
                .map(|(i, _)| *i)
                .collect();
            near.into_iter()
                .filter_map(|i| g.items.remove(&i).map(|d| (i, d)))
                .collect()
        };
        if taken.is_empty() {
            return Ok(());
        }
        self.broadcast(&packet(
            id::ENTITY_UPDATES,
            &proto::EntityUpdates {
                removed: Some(taken.iter().map(|(i, _)| *i).collect()),
                updates: None,
            },
        )?);
        for (_, d) in taken {
            self.add_item(player, &d.item, d.quantity)?;
        }
        Ok(())
    }

    /// Adds to the inventory (stacks first, hotbar then storage).
    pub fn add_item(&self, id: i32, item: &str, quantity: i32) -> Result<()> {
        self.with_inventory(id, |inv| inv.add(item, quantity))?;
        Ok(())
    }

    /// Adds a player; returns its network id and the outbox to drain. The
    /// outbox starts with the player list and the blocks changed so far.
    pub fn join(&self, name: &str, uuid: uuid::Uuid) -> (i32, mpsc::UnboundedReceiver<Packet>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let me = list_entry(name, uuid);
        let id = {
            let mut g = self.inner.lock().unwrap();
            g.next_id += 1;
            let id = g.next_id;
            for e in g.players.values() {
                if let Ok(p) = player_list_add(vec![me.clone()]) {
                    let _ = e.outbox.send(p);
                }
            }
            let everyone: Vec<_> = g
                .players
                .values()
                .map(|e| list_entry(&e.name, e.uuid))
                .chain([me])
                .collect();
            if let Ok(p) = player_list_add(everyone) {
                let _ = tx.send(p);
            }
            for (&(x, y, z), &b) in &g.placed {
                if let Ok(p) = set_block_packet(x, y, z, b) {
                    let _ = tx.send(p);
                }
            }
            g.players.insert(
                id,
                Entry {
                    name: name.to_owned(),
                    uuid,
                    outbox: tx,
                    position: None,
                    teleport_id: 0,
                    inv: self.saved_inventory(uuid).unwrap_or_default(),
                    creative: true,
                    view: None,
                    body: None,
                    look: None,
                    window: None,
                    next_window: 0,
                    consuming: HashMap::new(),
                },
            );
            id
        };
        self.broadcast_text(&format!("{name} joined"));
        (id, rx)
    }

    pub fn leave(&self, id: i32) {
        let gone = self.inner.lock().unwrap().players.remove(&id);
        if let Some(e) = gone {
            self.save_inventory(e.uuid, &e.inv);
            if let Some(p) = &e.position {
                self.save_position(e.uuid, p);
            }
            if e.view.is_some() {
                if let Ok(p) = packet(
                    id::ENTITY_UPDATES,
                    &proto::EntityUpdates {
                        removed: Some(vec![id]),
                        updates: None,
                    },
                ) {
                    self.broadcast(&p);
                }
            }
            if let Ok(p) = packet(
                id::REMOVE_FROM_PLAYER_LIST,
                &proto::RemoveFromServerPlayerList {
                    players: vec![e.uuid],
                },
            ) {
                self.broadcast(&p);
            }
            self.broadcast_text(&format!("{} left", e.name));
        }
    }

    /// A block id from a block key (any case) or a number.
    pub fn block_id(&self, name: &str) -> Option<i32> {
        name.parse()
            .ok()
            .or_else(|| self.blocks.get(&name.to_lowercase()).copied())
    }

    /// Puts `qty` of `item` in the hotbar slot that has it, else the first
    /// free one (or the last), and sends the inventory. Returns the slot.
    pub fn give(&self, id: i32, item: &str, qty: i32) -> Result<usize> {
        let slot = self.with_inventory(id, |inv| {
            let slot = (0..9)
                .find(|&i| {
                    inv.get(inventory::HOTBAR, i)
                        .is_some_and(|(x, _)| x == item)
                })
                .or_else(|| (0..9).find(|&i| inv.get(inventory::HOTBAR, i).is_none()))
                .unwrap_or(8);
            inv.set(inventory::HOTBAR, slot, Some((item.to_owned(), qty)));
            slot as usize
        })?;
        Ok(slot.unwrap_or(0))
    }

    /// Changes a player's inventory and sends it.
    pub fn with_inventory<T>(
        &self,
        id: i32,
        f: impl FnOnce(&mut Inventory) -> T,
    ) -> Result<Option<T>> {
        let (r, p) = {
            let mut g = self.inner.lock().unwrap();
            let Some(e) = g.players.get_mut(&id) else {
                return Ok(None);
            };
            let r = f(&mut e.inv);
            (r, packet(id::UPDATE_PLAYER_INVENTORY, &e.inv.packet())?)
        };
        self.send(id, p);
        Ok(Some(r))
    }

    /// Inventory packets from the client: moving stacks (`MoveItemStack`,
    /// `SmartMoveItemStack`) and dropping them (`DropItemStack`,
    /// `DropCreativeItem`) in front of the player.
    pub fn inventory_action(&self, id: i32, packet_id: u32, payload: &[u8]) -> Result<()> {
        match packet_id {
            self::id::MOVE_ITEM_STACK => {
                let (m, _) = proto::MoveItemStack::decode(payload, 0)?;
                self.with_inventory(id, |inv| {
                    inv.move_stack(
                        (m.from_section_id, m.from_slot_id),
                        m.quantity,
                        (m.to_section_id, m.to_slot_id),
                    )
                })?;
            }
            self::id::SMART_MOVE_ITEM_STACK => {
                let (m, _) = proto::SmartMoveItemStack::decode(payload, 0)?;
                self.with_inventory(id, |inv| {
                    inv.smart_move((m.from_section_id, m.from_slot_id), m.quantity)
                })?;
            }
            self::id::DROP_ITEM_STACK => {
                let (m, _) = proto::DropItemStack::decode(payload, 0)?;
                if let Some(Some((item, n))) = self.with_inventory(id, |inv| {
                    inv.take(m.inventory_section_id, m.slot_id, m.quantity)
                })? {
                    self.drop_in_front(id, &item, n)?;
                }
            }
            self::id::DROP_CREATIVE_ITEM => {
                let (m, _) = proto::DropCreativeItem::decode(payload, 0)?;
                self.drop_in_front(id, &m.item.item_id, m.item.quantity)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Drops an item a block and a half in front of a player.
    fn drop_in_front(&self, id: i32, item: &str, n: i32) -> Result<()> {
        let (p, yaw) = {
            let g = self.inner.lock().unwrap();
            let Some(e) = g.players.get(&id) else {
                return Ok(());
            };
            (e.position.clone(), e.look.as_ref().map_or(0.0, |d| d.yaw))
        };
        let Some(p) = p else { return Ok(()) };
        let (dx, dz) = (-(yaw as f64).sin() * 1.5, -(yaw as f64).cos() * 1.5);
        self.drop_item(item, n, p.x + dx, p.y + 1.0, p.z + dz)
    }

    /// `ClientOpenWindow`: the pocket crafting window (Tab), answered with
    /// `UpdateWindow` id 0 as the official server does.
    pub fn client_open_window(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::ClientOpenWindow::decode(payload, 0)?;
        if m.r#type != proto::WindowType::POCKET_CRAFTING {
            debug!("window type {} not opened", m.r#type.0);
            return Ok(());
        }
        {
            let mut g = self.inner.lock().unwrap();
            let Some(e) = g.players.get_mut(&id) else {
                return Ok(());
            };
            e.window = Some(crate::crafting::Window::fieldcraft());
        }
        let u = proto::UpdateWindow {
            id: 0,
            window_data: Some(self.book.fieldcraft_data()),
            inventory: None,
            extra_resources: None,
        };
        self.send(id, packet(self::id::UPDATE_WINDOW, &u)?);
        Ok(())
    }

    /// A player used the bench block at (x, y, z) (or a part of it next to
    /// it): opens its window (`SetPage` Bench, then `OpenWindow`).
    /// `false` when there is no bench there.
    pub fn open_bench(&self, id: i32, x: i32, y: i32, z: i32) -> Result<bool> {
        let found = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .find_map(|&(dx, dz)| {
                let b = self.block_in_world(x + dx, y, z + dz);
                let key = self.keys_by_id.get(&b)?;
                Some((
                    key.clone(),
                    self.book.bench(key)?.clone(),
                    (x + dx, y, z + dz),
                ))
            });
        let Some((key, bench, at)) = found else {
            return Ok(false);
        };
        let window_type = match proto::BenchType(bench.kind) {
            proto::BenchType::DIAGRAM_CRAFTING => proto::WindowType::DIAGRAM_CRAFTING,
            proto::BenchType::STRUCTURAL_CRAFTING => proto::WindowType::STRUCTURAL_CRAFTING,
            proto::BenchType::PROCESSING => proto::WindowType::PROCESSING,
            _ => proto::WindowType::BASIC_CRAFTING,
        };
        let wid = {
            let mut g = self.inner.lock().unwrap();
            let Some(e) = g.players.get_mut(&id) else {
                return Ok(true);
            };
            e.next_window = e.next_window.max(0) + 1;
            e.window = Some(crate::crafting::Window {
                id: e.next_window,
                bench: bench.clone(),
                block: Some((key.clone(), at)),
            });
            e.next_window
        };
        self.sound_2d(id, bench.open_sound)?;
        let page = proto::SetPage {
            page: proto::Page::BENCH,
            can_close_through_interaction: true,
        };
        self.send(id, packet(self::id::SET_PAGE, &page)?);
        let w = proto::OpenWindow {
            id: wid,
            window_type,
            window_data: Some(self.book.bench_data(&key, &bench)),
            inventory: None,
            extra_resources: Some(proto::ExtraResources { resources: None }),
        };
        self.send(id, packet(self::id::OPEN_WINDOW, &w)?);
        info!("opened {} ({}) for {id}", key, bench.id);
        Ok(true)
    }

    /// A UI sound for one player (none for index 0).
    fn sound_2d(&self, id: i32, index: i32) -> Result<()> {
        if index > 0 {
            let s = proto::PlaySoundEvent2D {
                sound_event_index: index,
                category: proto::SoundCategory::UI,
                volume_modifier: 1.0,
                pitch_modifier: 1.0,
            };
            self.send(id, packet(self::id::PLAY_SOUND_2D, &s)?);
        }
        Ok(())
    }

    /// `CloseWindow` from the client: forgotten and confirmed.
    pub fn close_window(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::CloseWindow::decode(payload, 0)?;
        let closed = {
            let mut g = self.inner.lock().unwrap();
            match g.players.get_mut(&id) {
                Some(e) if e.window.as_ref().is_some_and(|w| w.id == m.id) => e.window.take(),
                _ => None,
            }
        };
        self.send(id, packet(self::id::CLOSE_WINDOW, &m)?);
        if let Some(w) = closed {
            self.sound_2d(id, w.bench.close_sound)?;
        }
        Ok(())
    }

    /// `SendWindowAction`: crafting a recipe in the open window.
    pub fn window_action(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::SendWindowAction::decode(payload, 0)?;
        let proto::WindowAction::CraftRecipeAction(a) = &m.action else {
            debug!("window action not handled: {:?}", m.action);
            return Ok(());
        };
        let Some(recipe) = a.recipe_id.clone() else {
            return Ok(());
        };
        let (window, creative) = {
            let g = self.inner.lock().unwrap();
            let Some(e) = g.players.get(&id) else {
                return Ok(());
            };
            (e.window.clone(), e.creative)
        };
        let Some(w) = window.filter(|w| w.id == m.id) else {
            debug!("craft in window {} that is not open", m.id);
            return Ok(());
        };
        let bench = w.bench.id.clone();
        let made = self
            .with_inventory(id, |inv| {
                let r = self
                    .book
                    .craft(inv, &recipe, a.quantity, w.bench.kind, &bench, creative);
                if let Ok(out) = &r {
                    let left: Vec<(String, i32)> = out
                        .iter()
                        .map(|(item, n)| (item.clone(), inv.add(item, *n)))
                        .filter(|(_, n)| *n > 0)
                        .collect();
                    return Ok((out.clone(), left));
                }
                r.map(|o| (o, Vec::new()))
            })?
            .unwrap_or(Err(crate::crafting::Refused::Missing));
        match made {
            Ok((out, left)) => {
                info!("{id} crafted {recipe} ×{}: {out:?}", a.quantity);
                for (item, n) in left {
                    self.drop_in_front(id, &item, n)?;
                }
                match &w.block {
                    // At a bench: its sound, where it stands, for everyone.
                    Some((_, (x, y, z))) if w.bench.completed_sound > 0 => {
                        let s = proto::PlaySoundEvent3D {
                            sound_event_index: w.bench.completed_sound,
                            category: proto::SoundCategory::SFX,
                            position: Some(proto::Position {
                                x: *x as f64 + 0.5,
                                y: *y as f64 + 0.5,
                                z: *z as f64 + 0.5,
                            }),
                            volume_modifier: 1.0,
                            pitch_modifier: 1.0,
                        };
                        self.broadcast(&packet(self::id::PLAY_SOUND_3D, &s)?);
                    }
                    Some(_) => {}
                    None => {
                        let s = self.book.sound("SFX_Player_Craft_Item_Inventory");
                        self.sound_2d(id, s)?;
                    }
                }
            }
            Err(crate::crafting::Refused::Unknown) => {
                warn!("{id} asked for unknown recipe {recipe}")
            }
            Err(e) => info!("{id} cannot craft {recipe} at {bench}: {e:?}"),
        }
        Ok(())
    }

    /// The creative inventory: `SetCreativeItem` puts an item in any slot,
    /// `SmartGiveCreativeItem` gives it to the hotbar.
    pub fn creative_item(&self, id: i32, packet_id: u32, payload: &[u8]) -> Result<()> {
        if packet_id == self::id::SET_CREATIVE_ITEM {
            let (m, _) = proto::SetCreativeItem::decode(payload, 0)?;
            self.with_inventory(id, |inv| {
                inv.set(
                    m.inventory_section_id,
                    m.slot_id,
                    Some((m.item.item_id.clone(), m.item.quantity)),
                )
            })?;
        } else {
            let (m, _) = proto::SmartGiveCreativeItem::decode(payload, 0)?;
            self.give(id, &m.item.item_id, m.item.quantity)?;
        }
        Ok(())
    }

    fn inventory_file(&self, uuid: uuid::Uuid) -> Option<std::path::PathBuf> {
        Some(
            self.save
                .as_ref()?
                .with_file_name("inventories")
                .join(format!("{uuid}.txt")),
        )
    }

    fn saved_inventory(&self, uuid: uuid::Uuid) -> Option<Inventory> {
        let text = std::fs::read_to_string(self.inventory_file(uuid)?).ok()?;
        Some(Inventory::from_text(&text))
    }

    fn save_inventory(&self, uuid: uuid::Uuid, inv: &Inventory) {
        let Some(f) = self.inventory_file(uuid) else {
            return;
        };
        let r = std::fs::create_dir_all(f.parent().unwrap())
            .and_then(|_| std::fs::write(&f, inv.to_text()));
        if let Err(e) = r {
            tracing::warn!("saving an inventory failed: {e}");
        }
    }

    /// A few building blocks in the hotbar (those the registry has), for a
    /// player with nothing; a returning player gets its saved inventory.
    pub fn starter_kit(&self, id: i32) -> Result<()> {
        let empty = self
            .inner
            .lock()
            .unwrap()
            .players
            .get(&id)
            .is_some_and(|e| e.inv.to_text().is_empty());
        if !empty {
            return self.with_inventory(id, |_| ()).map(|_| ());
        }
        let mut any = false;
        for b in STARTER {
            if self.blocks.contains_key(&b.to_lowercase()) {
                let key = self.names.get(&b.to_lowercase()).map_or(*b, String::as_str);
                self.give(id, key, 100)?;
                any = true;
            }
        }
        if !any {
            tracing::warn!("no starter blocks found in the registry");
        }
        Ok(())
    }

    /// Block keys containing `part`, sorted.
    pub fn find_blocks(&self, part: &str) -> Vec<String> {
        let part = part.to_lowercase();
        let mut v: Vec<String> = self
            .blocks
            .keys()
            .filter(|k| k.contains(&part))
            .cloned()
            .collect();
        v.sort();
        v
    }

    /// Changes one block for everyone, now and for later joiners.
    pub fn set_block(&self, x: i32, y: i32, z: i32, block: i32) -> Result<()> {
        let p = set_block_packet(x, y, z, block)?;
        self.inner.lock().unwrap().placed.insert((x, y, z), block);
        self.broadcast(&p);
        self.save_change(x, y, z, block);
        Ok(())
    }

    /// Keeps changed blocks in `<world>/blocks.txt`, one line per change
    /// (`x y z key`; the last line for a position wins). Blocks are saved
    /// by key, since ids move when the assets change.
    pub fn with_world(mut self, dir: std::path::PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("blocks.txt");
        if let Ok(text) = std::fs::read_to_string(&file) {
            let mut n = 0;
            let mut g = self.inner.lock().unwrap();
            for line in text.lines() {
                let w: Vec<&str> = line.split_whitespace().collect();
                let [x, y, z, key] = w[..] else { continue };
                let (Ok(x), Ok(y), Ok(z)) = (x.parse(), y.parse(), z.parse()) else {
                    continue;
                };
                let id = if key == "air" {
                    Some(0)
                } else {
                    self.blocks.get(&key.to_lowercase()).copied()
                };
                if let Some(id) = id {
                    g.placed.insert((x, y, z), id);
                    n += 1;
                }
            }
            drop(g);
            info!("loaded {n} block changes from {}", file.display());
        }
        self.save = Some(file);
        Ok(self)
    }

    /// Where a player was when it last left (`<world>/players.txt`, one
    /// line per leave: `uuid x y z`; the last one wins).
    pub fn saved_position(&self, uuid: uuid::Uuid) -> Option<proto::Position> {
        let file = self.save.as_ref()?.with_file_name("players.txt");
        let text = std::fs::read_to_string(file).ok()?;
        let u = uuid.to_string();
        text.lines().rev().find_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w[..] {
                [id, x, y, z] if id == u => Some(proto::Position {
                    x: x.parse().ok()?,
                    y: y.parse().ok()?,
                    z: z.parse().ok()?,
                }),
                _ => None,
            }
        })
    }

    fn save_position(&self, uuid: uuid::Uuid, p: &proto::Position) {
        let Some(file) = self.save.as_ref().map(|f| f.with_file_name("players.txt")) else {
            return;
        };
        use std::io::Write;
        let r = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut f| writeln!(f, "{uuid} {} {} {}", p.x, p.y, p.z));
        if let Err(e) = r {
            tracing::warn!("saving a position failed: {e}");
        }
    }

    fn save_change(&self, x: i32, y: i32, z: i32, block: i32) {
        let Some(file) = &self.save else { return };
        let key = if block == 0 {
            "air".to_owned()
        } else {
            match self.blocks.iter().find(|(_, i)| **i == block) {
                Some((k, _)) => self.names.get(k).cloned().unwrap_or_else(|| k.clone()),
                None => return,
            }
        };
        use std::io::Write;
        let r = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut f| writeln!(f, "{x} {y} {z} {key}"));
        if let Err(e) = r {
            tracing::warn!("saving a block change failed: {e}");
        }
    }

    pub fn names(&self) -> Vec<String> {
        let g = self.inner.lock().unwrap();
        g.players.values().map(|e| e.name.clone()).collect()
    }

    pub fn position(&self, id: i32) -> Option<proto::Position> {
        let g = self.inner.lock().unwrap();
        g.players.get(&id).and_then(|e| e.position.clone())
    }

    pub fn send(&self, id: i32, p: Packet) {
        if let Some(e) = self.inner.lock().unwrap().players.get(&id) {
            let _ = e.outbox.send(p);
        }
    }

    pub fn broadcast(&self, p: &Packet) {
        for e in self.inner.lock().unwrap().players.values() {
            let _ = e.outbox.send(p.clone());
        }
    }

    pub fn broadcast_text(&self, text: &str) {
        info!("[chat] {text}");
        if let Ok(p) = server_message(text) {
            self.broadcast(&p);
        }
    }

    fn tell(&self, id: i32, text: &str) {
        if let Ok(p) = server_message(text) {
            self.send(id, p);
        }
    }

    /// Reads a `ClientMovement` and keeps the player's position.
    pub fn movement(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::ClientMovement::decode(payload, 0)?;
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.players.get_mut(&id) else {
            return Ok(());
        };
        if let Some(ack) = &m.teleport_ack
            && ack.teleport_id == e.teleport_id
        {
            tracing::debug!("{} acknowledged teleport {}", e.name, ack.teleport_id);
        }
        // Absolute, or a delta in 1/10000 of a block.
        if let Some(p) = m.absolute_position {
            e.position = Some(p);
        } else if let (Some(d), Some(p)) = (&m.relative_position, e.position.as_mut())
            && (d.x, d.y, d.z) != (0, 0, 0)
        {
            p.x += d.x as f64 / 10000.0;
            p.y += d.y as f64 / 10000.0;
            p.z += d.z as f64 / 10000.0;
        }
        if m.body_orientation.is_some() {
            e.body = m.body_orientation.clone();
        }
        if m.look_orientation.is_some() {
            e.look = m.look_orientation.clone();
        }
        if e.view.is_none() {
            return Ok(());
        }
        // Others see it move.
        let mut parts = vec![proto::ComponentUpdate::TransformUpdate(Box::new(
            proto::TransformUpdate {
                transform: proto::ModelTransform {
                    position: e.position.clone(),
                    body_orientation: e.body.clone(),
                    look_orientation: e.look.clone(),
                },
            },
        ))];
        if let Some(ms) = m.movement_states {
            parts.push(proto::ComponentUpdate::MovementStatesUpdate(Box::new(
                proto::MovementStatesUpdate {
                    movement_states: ms,
                },
            )));
        }
        let p = packet(
            id::ENTITY_UPDATES,
            &proto::EntityUpdates {
                removed: None,
                updates: Some(vec![proto::EntityUpdate {
                    network_id: id,
                    removed: None,
                    updates: Some(parts),
                }]),
            },
        )?;
        for (other, o) in &g.players {
            if *other != id && o.view.is_some() {
                let _ = o.outbox.send(p.clone());
            }
        }
        drop(g);
        self.pick_up(id)
    }

    /// A player is in the world: the others get its entity (`view`, as the
    /// official server sends another player: model, name plate, hitbox,
    /// equipment, UI, movement states, transform, effects, stats) and it gets
    /// theirs.
    pub fn enter_world(&self, id: i32, view: proto::EntityUpdate) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.players.get_mut(&id) else {
            return Ok(());
        };
        e.view = Some(view.clone());
        let mine = packet(
            id::ENTITY_UPDATES,
            &proto::EntityUpdates {
                removed: None,
                updates: Some(vec![view]),
            },
        )?;
        let mut theirs = Vec::new();
        for (other, o) in &g.players {
            if *other == id {
                continue;
            }
            if let Some(v) = &o.view {
                let _ = o.outbox.send(mine.clone());
                let mut v = v.clone();
                // Where it is now.
                for c in v.updates.iter_mut().flatten() {
                    if let proto::ComponentUpdate::TransformUpdate(t) = c {
                        t.transform.position = o.position.clone().or(t.transform.position.take());
                        if o.body.is_some() {
                            t.transform.body_orientation = o.body.clone();
                        }
                        if o.look.is_some() {
                            t.transform.look_orientation = o.look.clone();
                        }
                    }
                }
                theirs.push(v);
            }
        }
        // Items lying around.
        theirs.extend(g.items.iter().map(|(n, d)| item_entity(*n, d)));
        if !theirs.is_empty() {
            let p = packet(
                id::ENTITY_UPDATES,
                &proto::EntityUpdates {
                    removed: None,
                    updates: Some(theirs),
                },
            )?;
            let _ = g.players[&id].outbox.send(p);
        }
        Ok(())
    }

    /// Reads a `SyncInteractionChains` and changes blocks, simplified as in
    /// creative mode: a new primary chain on a block breaks it at once; a
    /// placement the client reports (an interaction step with a placed block
    /// id and position, after its own prediction) is made for everyone.
    /// Returns the changes, for the caller to assert again a little later:
    /// the client rolls its prediction back when the chain is not answered,
    /// which Aerie does not do yet.
    pub fn interactions(&self, _player: i32, payload: &[u8]) -> Result<Vec<BlockChange>> {
        let (m, _) = proto::SyncInteractionChains::decode(payload, 0)?;
        let mut changes = Vec::new();
        let mut hits = Vec::new();
        let mut uses = Vec::new();
        let mut walk = |c: &proto::SyncInteractionChain| -> Result<()> {
            if c.initial
                && c.forked_id.is_none()
                && c.interaction_type == proto::InteractionType::PRIMARY
            {
                if let Some(t) = c.data.as_ref().and_then(|d| d.block_position.as_ref()) {
                    hits.push(((t.x, t.y, t.z), c.item_in_hand_id.clone()));
                }
            }
            if c.initial
                && c.forked_id.is_none()
                && c.interaction_type == proto::InteractionType::USE
            {
                if let Some(t) = c.data.as_ref().and_then(|d| d.block_position.as_ref()) {
                    uses.push((t.x, t.y, t.z));
                }
            }
            for d in c.interaction_data.iter().flatten().flatten() {
                if let (Some(p), true) = (&d.block_position, d.placed_block_id != i32::MIN) {
                    changes.push(((p.x, p.y, p.z), d.placed_block_id));
                }
            }
            Ok(())
        };
        for c in &m.updates {
            self.consume_step(_player, c)?;
            walk(c)?;
            for f in c.new_forks.iter().flatten() {
                walk(f)?;
            }
        }
        for (x, y, z) in uses {
            self.open_bench(_player, x, y, z)?;
        }
        for ((x, y, z), held) in hits {
            if let Some(c) = self.hit(_player, x, y, z, held.as_deref())? {
                changes.push(c);
            }
        }
        changes.dedup();
        for &((x, y, z), b) in &changes {
            if self.block_at(x, y, z) != Some(b) {
                self.set_block(x, y, z, b)?;
            }
        }
        Ok(changes)
    }

    /// The changed blocks inside section (cx, cy, cz), as (x, y, z within
    /// the section, block id).
    pub fn changes_in(&self, cx: i32, cy: i32, cz: i32) -> Vec<(usize, usize, usize, i32)> {
        let g = self.inner.lock().unwrap();
        g.placed
            .iter()
            .filter(|((x, y, z), _)| {
                x.div_euclid(32) == cx && y.div_euclid(32) == cy && z.div_euclid(32) == cz
            })
            .map(|(&(x, y, z), &b)| {
                (
                    x.rem_euclid(32) as usize,
                    y.rem_euclid(32) as usize,
                    z.rem_euclid(32) as usize,
                    b,
                )
            })
            .collect()
    }

    /// A block changed since start, if any.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<i32> {
        self.inner.lock().unwrap().placed.get(&(x, y, z)).copied()
    }

    /// Moves a player (a `ClientTeleport` with a new id).
    pub fn teleport(&self, id: i32, to: proto::Position) -> Result<()> {
        let teleport_id = {
            let mut g = self.inner.lock().unwrap();
            let Some(e) = g.players.get_mut(&id) else {
                return Ok(());
            };
            e.teleport_id = e.teleport_id.wrapping_add(1);
            e.position = Some(to.clone());
            e.teleport_id
        };
        self.send(id, teleport_packet(teleport_id, to)?);
        Ok(())
    }

    /// A `ChatMessage` from a player: a command, or a line for everyone.
    pub fn chat(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::ChatMessage::decode(payload, 0)?;
        let text = m.message.unwrap_or_default();
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        let name = {
            let g = self.inner.lock().unwrap();
            match g.players.get(&id) {
                Some(e) => e.name.clone(),
                None => return Ok(()),
            }
        };
        match text.strip_prefix('/') {
            Some(cmd) => {
                info!("{name} ran /{cmd}");
                self.command(id, cmd)
            }
            None => {
                self.broadcast_text(&format!("<{name}> {text}"));
                Ok(())
            }
        }
    }

    fn command(&self, id: i32, line: &str) -> Result<()> {
        let mut words = line.split_whitespace();
        let name = words.next().unwrap_or("");
        let args: Vec<&str> = words.collect();
        match (name, args.as_slice()) {
            ("help", _) => self.tell(
                id,
                "Commands: /help, /list, /pos, /tp <x> <y> <z>, /gamemode <creative|adventure>, \
                 /setblock <x> <y> <z> <block>, /fill <x1> <y1> <z1> <x2> <y2> <z2> <block>, \
                 /blocks <part of a name>, /give <item> [amount], /clear",
            ),
            ("clear", []) => {
                self.with_inventory(id, |inv| *inv = Inventory::default())?;
                self.tell(id, "Inventory cleared");
            }
            ("give", [item, rest @ ..]) => {
                let qty = rest.first().and_then(|q| q.parse().ok()).unwrap_or(100);
                // Block keys are matched case-insensitively; anything else is
                // sent as typed (items that are not blocks).
                let key = self
                    .names
                    .get(&item.to_lowercase())
                    .cloned()
                    .unwrap_or_else(|| item.to_string());
                let slot = self.give(id, &key, qty)?;
                self.tell(id, &format!("Gave {qty} {key} (hotbar slot {})", slot + 1));
            }
            ("blocks", [part]) => {
                let found = self.find_blocks(part);
                if found.is_empty() {
                    self.tell(id, &format!("No block matches {part}"));
                    return Ok(());
                }
                let shown: Vec<&str> = found.iter().take(15).map(String::as_str).collect();
                let more = found.len().saturating_sub(shown.len());
                self.tell(
                    id,
                    &format!(
                        "{} blocks: {}{}",
                        found.len(),
                        shown.join(", "),
                        if more > 0 {
                            format!(" and {more} more")
                        } else {
                            String::new()
                        }
                    ),
                );
            }
            ("setblock", [x, y, z, block]) => {
                let here = self.position(id);
                match (block_coords(here.as_ref(), [x, y, z]), self.block_id(block)) {
                    (Some([x, y, z]), Some(b)) => {
                        self.set_block(x, y, z, b)?;
                        self.tell(id, &format!("Set {x} {y} {z} to {block}"));
                    }
                    (None, _) => self.tell(id, "Usage: /setblock <x> <y> <z> <block>"),
                    (_, None) => self.tell(id, &format!("No block {block}; try /blocks <name>")),
                }
            }
            ("fill", [x1, y1, z1, x2, y2, z2, block]) => {
                let here = self.position(id);
                let a = block_coords(here.as_ref(), [x1, y1, z1]);
                let b = block_coords(here.as_ref(), [x2, y2, z2]);
                match (a, b, self.block_id(block)) {
                    (Some(a), Some(b), Some(blk)) => {
                        let lo = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
                        let hi = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
                        let n: i64 = (0..3).map(|i| (hi[i] - lo[i]) as i64 + 1).product();
                        if n > FILL_MAX {
                            self.tell(id, &format!("{n} blocks is too many (at most {FILL_MAX})"));
                        } else {
                            for x in lo[0]..=hi[0] {
                                for y in lo[1]..=hi[1] {
                                    for z in lo[2]..=hi[2] {
                                        self.set_block(x, y, z, blk)?;
                                    }
                                }
                            }
                            self.tell(id, &format!("Filled {n} blocks with {block}"));
                        }
                    }
                    (_, _, None) => self.tell(id, &format!("No block {block}; try /blocks <name>")),
                    _ => self.tell(id, "Usage: /fill <x1> <y1> <z1> <x2> <y2> <z2> <block>"),
                }
            }
            ("list", _) => {
                let names = self.names();
                self.tell(id, &format!("{} online: {}", names.len(), names.join(", ")));
            }
            ("pos", _) => match self.position(id) {
                Some(p) => self.tell(id, &format!("You are at {:.1} {:.1} {:.1}", p.x, p.y, p.z)),
                None => self.tell(id, "Your position is not known yet"),
            },
            ("tp", [x, y, z]) => {
                let here = self.position(id);
                let coord = |s: &str, base: Option<f64>| -> Option<f64> {
                    match s.strip_prefix('~') {
                        Some(rest) => {
                            let d = if rest.is_empty() {
                                0.0
                            } else {
                                rest.parse().ok()?
                            };
                            Some(base? + d)
                        }
                        None => s.parse().ok(),
                    }
                };
                let to = (
                    coord(x, here.as_ref().map(|p| p.x)),
                    coord(y, here.as_ref().map(|p| p.y)),
                    coord(z, here.as_ref().map(|p| p.z)),
                );
                match to {
                    (Some(x), Some(y), Some(z)) => {
                        self.teleport(id, proto::Position { x, y, z })?;
                        self.tell(id, &format!("Teleported to {x:.1} {y:.1} {z:.1}"));
                    }
                    _ => self.tell(id, "Usage: /tp <x> <y> <z> (numbers, or ~ for relative)"),
                }
            }
            ("gamemode", [mode]) => {
                let m = match mode.to_ascii_lowercase().as_str() {
                    "creative" | "c" | "1" => Some(proto::GameMode::CREATIVE),
                    "adventure" | "a" | "0" => Some(proto::GameMode::ADVENTURE),
                    _ => None,
                };
                match m {
                    Some(game_mode) => {
                        self.set_creative(id, game_mode == proto::GameMode::CREATIVE);
                        self.send(
                            id,
                            packet(id::SET_GAME_MODE, &proto::SetGameMode { game_mode })?,
                        );
                        self.tell(id, &format!("Game mode set to {mode}"));
                    }
                    None => self.tell(id, "Usage: /gamemode <creative|adventure>"),
                }
            }
            _ => self.tell(id, &format!("Unknown command /{name}; try /help")),
        }
        Ok(())
    }
}

/// Block coordinates from command words: numbers, or `~` / `~n` relative to
/// the player's position.
fn block_coords(here: Option<&proto::Position>, words: [&&str; 3]) -> Option<[i32; 3]> {
    let base = here.map(|p| [p.x, p.y, p.z]);
    let mut out = [0; 3];
    for i in 0..3 {
        let v = match words[i].strip_prefix('~') {
            Some(rest) => {
                let d: f64 = if rest.is_empty() {
                    0.0
                } else {
                    rest.parse().ok()?
                };
                base?[i] + d
            }
            None => words[i].parse().ok()?,
        };
        out[i] = v.floor() as i32;
    }
    Some(out)
}

fn list_entry(name: &str, uuid: uuid::Uuid) -> proto::ServerPlayerListPlayer {
    proto::ServerPlayerListPlayer {
        uuid,
        username: Some(name.to_owned()),
        world_uuid: Some(crate::join::WORLD_UUID),
        ping: 0,
        spectating: false,
        lives_remaining: None,
    }
}

fn player_list_add(players: Vec<proto::ServerPlayerListPlayer>) -> Result<Packet> {
    packet(
        id::ADD_TO_PLAYER_LIST,
        &proto::AddToServerPlayerList { players },
    )
}

pub fn set_block_packet(x: i32, y: i32, z: i32, block_id: i32) -> Result<Packet> {
    packet(
        id::SERVER_SET_BLOCK,
        &proto::ServerSetBlock {
            x,
            y,
            z,
            block_id,
            filler: 0,
            rotation: 0,
        },
    )
}

/// A plain chat line.
pub fn server_message(text: &str) -> Result<Packet> {
    packet(
        id::SERVER_MESSAGE,
        &proto::ServerMessage {
            r#type: proto::ChatType::CHAT,
            message: Some(proto::FormattedMessage {
                raw_text: Some(text.to_owned()),
                ..Default::default()
            }),
        },
    )
}

/// A dropped item's entity: where it lies and what it is.
fn item_entity(nid: i32, d: &Dropped) -> proto::EntityUpdate {
    proto::EntityUpdate {
        network_id: nid,
        removed: None,
        updates: Some(vec![
            proto::ComponentUpdate::TransformUpdate(Box::new(proto::TransformUpdate {
                transform: proto::ModelTransform {
                    position: Some(d.at.clone()),
                    body_orientation: None,
                    look_orientation: None,
                },
            })),
            proto::ComponentUpdate::ItemUpdate(Box::new(proto::ItemUpdate {
                item: proto::ItemWithAllMetadata {
                    item_id: d.item.clone(),
                    quantity: d.quantity,
                    durability: 0.0,
                    max_durability: 0.0,
                    quality: -1,
                    override_dropped_item_animation: false,
                    metadata: None,
                },
                entity_scale: 1.0,
            })),
        ]),
    }
}

pub fn teleport_packet(teleport_id: i8, to: proto::Position) -> Result<Packet> {
    packet(
        id::CLIENT_TELEPORT,
        &proto::ClientTeleport {
            teleport_id,
            ignored_transform_fields: 0,
            relative_transform_fields: 0,
            model_transform: proto::ModelTransform {
                position: Some(to),
                body_orientation: None,
                look_orientation: None,
            },
            reset_velocity: true,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(text: &str) -> Vec<u8> {
        packet(
            id::CHAT_MESSAGE,
            &proto::ChatMessage {
                message: Some(text.into()),
            },
        )
        .unwrap()
        .1
    }

    fn texts(rx: &mut mpsc::UnboundedReceiver<Packet>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok((i, p)) = rx.try_recv() {
            if i == id::SERVER_MESSAGE {
                let (m, _) = proto::ServerMessage::decode(&p, 0).unwrap();
                out.push(m.message.unwrap().raw_text.unwrap());
            } else {
                out.push(format!("#{i}"));
            }
        }
        out
    }

    #[test]
    fn chat_reaches_everyone() {
        let hub = Hub::default();
        let (a, mut ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        let (_b, mut rb) = hub.join("Bia", uuid::Uuid::from_u128(2));
        texts(&mut ra);
        texts(&mut rb);
        hub.chat(a, &chat("oi")).unwrap();
        assert_eq!(texts(&mut ra), ["<Ana> oi"]);
        assert_eq!(texts(&mut rb), ["<Ana> oi"]);
    }

    #[test]
    fn commands_answer_only_the_sender() {
        let hub = Hub::default();
        let (a, mut ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        let (_b, mut rb) = hub.join("Bia", uuid::Uuid::from_u128(2));
        texts(&mut ra);
        texts(&mut rb);
        hub.chat(a, &chat("/list")).unwrap();
        assert_eq!(texts(&mut ra), ["2 online: Ana, Bia"]);
        assert!(texts(&mut rb).is_empty());
    }

    #[test]
    fn tp_relative_and_pos() {
        let hub = Hub::default();
        let (a, mut ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        hub.chat(a, &chat("/tp ~ 10 ~")).unwrap();
        assert_eq!(
            texts(&mut ra).last().unwrap(),
            "Usage: /tp <x> <y> <z> (numbers, or ~ for relative)"
        );
        hub.chat(a, &chat("/tp 1 2 3")).unwrap();
        assert_eq!(texts(&mut ra), ["#109", "Teleported to 1.0 2.0 3.0"]);
        hub.chat(a, &chat("/tp ~5 ~ ~-1")).unwrap();
        assert_eq!(texts(&mut ra), ["#109", "Teleported to 6.0 2.0 2.0"]);
        hub.chat(a, &chat("/pos")).unwrap();
        assert_eq!(texts(&mut ra), ["You are at 6.0 2.0 2.0"]);
    }

    #[test]
    fn movement_keeps_absolute_position() {
        let hub = Hub::default();
        let (a, _ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        let m = proto::ClientMovement {
            absolute_position: Some(proto::Position {
                x: 4.0,
                y: 70.0,
                z: -2.0,
            }),
            ..Default::default()
        };
        hub.movement(a, &packet(id::CLIENT_MOVEMENT, &m).unwrap().1)
            .unwrap();
        assert_eq!(hub.position(a).unwrap().y, 70.0);
    }

    #[test]
    fn blocks_reach_everyone_and_later_joiners() {
        let hub = Hub::new(HashMap::from([("rock_stone".to_owned(), 7)]));
        let (a, mut ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        texts(&mut ra);
        hub.chat(a, &chat("/setblock 1 2 3 Rock_Stone")).unwrap();
        assert_eq!(texts(&mut ra), ["#140", "Set 1 2 3 to Rock_Stone"]);
        hub.chat(a, &chat("/setblock 1 2 3 Nope")).unwrap();
        assert_eq!(texts(&mut ra), ["No block Nope; try /blocks <name>"]);
        hub.chat(a, &chat("/fill 0 0 0 1 1 0 7")).unwrap();
        assert_eq!(texts(&mut ra).last().unwrap(), "Filled 4 blocks with 7");
        let (_b, mut rb) = hub.join("Bia", uuid::Uuid::from_u128(2));
        // Player list, then the 5 changed blocks, then the greeting.
        let got = texts(&mut rb);
        assert_eq!(got[0], "#224");
        assert_eq!(got.iter().filter(|t| *t == "#140").count(), 5);
        hub.chat(a, &chat("/fill 0 0 0 100 100 100 7")).unwrap();
        assert!(texts(&mut ra).last().unwrap().contains("too many"));
    }

    #[test]
    fn leaving_is_announced() {
        let hub = Hub::default();
        let (a, _ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        let (_b, mut rb) = hub.join("Bia", uuid::Uuid::from_u128(2));
        texts(&mut rb);
        hub.leave(a);
        assert_eq!(texts(&mut rb), ["#225", "Ana left"]);
        assert_eq!(hub.names(), ["Bia"]);
    }
}

#[cfg(test)]
mod give_tests {
    use super::*;

    #[test]
    fn give_fills_slots_and_sends_inventory() {
        let hub = Hub::new(HashMap::from([("rock_stone".to_owned(), 7)]));
        let (a, mut ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        while ra.try_recv().is_ok() {}
        hub.starter_kit(a).unwrap();
        let (i, p) = ra.try_recv().unwrap();
        assert_eq!(i, id::UPDATE_PLAYER_INVENTORY);
        let (inv, _) = proto::UpdatePlayerInventory::decode(&p, 0).unwrap();
        let hb = inv.hotbar.unwrap().items.unwrap();
        assert_eq!(hb[0].1.item_id, "Rock_Stone");
        assert_eq!(hub.give(a, "Torch", 5).unwrap(), 1);
        assert_eq!(hub.give(a, "Rock_Stone", 3).unwrap(), 0);
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;

    fn sync(
        t: proto::InteractionType,
        initial: bool,
        placed: Option<(i32, i32, i32, i32)>,
    ) -> Vec<u8> {
        let step = proto::InteractionSyncData {
            block_position: placed.map(|(x, y, z, _)| proto::BlockPosition { x, y, z }),
            placed_block_id: placed.map_or(i32::MIN, |p| p.3),
            ..Default::default()
        };
        let c = proto::SyncInteractionChain {
            initial,
            interaction_type: t,
            data: Some(proto::InteractionChainData {
                block_position: Some(proto::BlockPosition {
                    x: 18,
                    y: 63,
                    z: 16,
                }),
                ..Default::default()
            }),
            interaction_data: Some(vec![Some(step)]),
            ..Default::default()
        };
        packet(
            id::SYNC_INTERACTION_CHAINS,
            &proto::SyncInteractionChains { updates: vec![c] },
        )
        .unwrap()
        .1
    }

    fn set_blocks(rx: &mut mpsc::UnboundedReceiver<Packet>) -> Vec<(i32, i32, i32, i32)> {
        let mut out = Vec::new();
        while let Ok((i, p)) = rx.try_recv() {
            if i == id::SERVER_SET_BLOCK {
                let (b, _) = proto::ServerSetBlock::decode(&p, 0).unwrap();
                out.push((b.x, b.y, b.z, b.block_id));
            }
        }
        out
    }

    #[test]
    fn break_and_place() {
        let hub = Hub::new(HashMap::new());
        let (a, mut ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        hub.interactions(a, &sync(proto::InteractionType::PRIMARY, true, None))
            .unwrap();
        assert_eq!(set_blocks(&mut ra), [(18, 63, 16, 0)]);
        // Holding the button sends more of the same chain: nothing new.
        hub.interactions(a, &sync(proto::InteractionType::PRIMARY, false, None))
            .unwrap();
        assert!(set_blocks(&mut ra).is_empty());
        // The client reports where it placed a block.
        let c = hub
            .interactions(
                a,
                &sync(
                    proto::InteractionType::SECONDARY,
                    false,
                    Some((17, 64, 14, 1530)),
                ),
            )
            .unwrap();
        assert_eq!(c, [((17, 64, 14), 1530)]);
        assert_eq!(set_blocks(&mut ra), [(17, 64, 14, 1530)]);
        // Reported again: already there, not sent again.
        hub.interactions(
            a,
            &sync(
                proto::InteractionType::SECONDARY,
                false,
                Some((17, 64, 14, 1530)),
            ),
        )
        .unwrap();
        assert!(set_blocks(&mut ra).is_empty());
    }
}

/// The recorded interactions of a real client (`AERIE_INTERACTIONS=<dir of
/// *-290-*.bin>`, `cargo test -p aerie-server recorded -- --ignored`):
/// lists the block changes Aerie makes from them.
#[test]
#[ignore]
fn recorded_interactions() {
    let dir = std::env::var("AERIE_INTERACTIONS").expect("AERIE_INTERACTIONS");
    let hub = Hub::default();
    let (a, _rx) = hub.join("Ana", uuid::Uuid::from_u128(1));
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains("-290-"))
        .collect();
    files.sort();
    for f in files {
        let c = hub.interactions(a, &std::fs::read(&f).unwrap()).unwrap();
        if !c.is_empty() {
            eprintln!("{}: {c:?}", f.file_name().unwrap().to_string_lossy());
        }
    }
}

/// Per primary chain of a recording: the states of the steps that name a
/// block, in order (study aid, `AERIE_INTERACTIONS`, `cargo test -p
/// aerie-server primary_steps -- --ignored --nocapture`).
#[test]
#[ignore]
fn primary_steps() {
    let dir = std::env::var("AERIE_INTERACTIONS").expect("AERIE_INTERACTIONS");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains("-290-"))
        .collect();
    files.sort();
    let mut by: std::collections::BTreeMap<i32, Vec<String>> = Default::default();
    fn walk(
        c: &proto::SyncInteractionChain,
        by: &mut std::collections::BTreeMap<i32, Vec<String>>,
        f: &str,
    ) {
        if c.interaction_type == proto::InteractionType::PRIMARY {
            for d in c.interaction_data.iter().flatten().flatten() {
                if let Some(p) = &d.block_position {
                    by.entry(c.chain_id).or_default().push(format!(
                        "{f}:s{}r{}@{},{},{}{}",
                        d.state.0,
                        d.root_interaction,
                        p.x,
                        p.y,
                        p.z,
                        if c.forked_id.is_some() { "F" } else { "" }
                    ));
                }
            }
        }
        for k in c.new_forks.iter().flatten() {
            walk(k, by, f);
        }
    }
    for f in files {
        let (m, _) = proto::SyncInteractionChains::decode(&std::fs::read(&f).unwrap(), 0).unwrap();
        let n = f.file_name().unwrap().to_string_lossy()[..4].to_string();
        for c in &m.updates {
            walk(c, &mut by, &n);
        }
    }
    for (k, v) in by {
        eprintln!("chain {k}: {}", v.join(" "));
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[test]
    fn changes_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("aerie-world-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let blocks = HashMap::from([("rock_stone".to_owned(), 7)]);
        let names = vec!["Rock_Stone".to_owned()];
        let hub = Hub::with_names(blocks.clone(), names.clone())
            .with_world(dir.clone())
            .unwrap();
        hub.set_block(1, 2, 3, 7).unwrap();
        hub.set_block(4, 5, 6, 0).unwrap();
        let again = Hub::with_names(blocks, names)
            .with_world(dir.clone())
            .unwrap();
        assert_eq!(again.block_at(1, 2, 3), Some(7));
        assert_eq!(again.block_at(4, 5, 6), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
