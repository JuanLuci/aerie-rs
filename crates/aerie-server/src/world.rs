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

use crate::inventory::{self, Inventory, Stack};

/// Packet ids used here (gen/packets.tsv).
pub mod id {
    pub const SET_GAME_MODE: u32 = 101;
    pub const CLIENT_MOVEMENT: u32 = 108;
    pub const CLIENT_TELEPORT: u32 = 109;
    pub const SERVER_MESSAGE: u32 = 210;
    pub const SERVER_SET_BLOCK: u32 = 140;
    pub const CHAT_MESSAGE: u32 = 211;
    pub const CUSTOM_HUD: u32 = 217;
    pub const SYNC_INTERACTION_CHAINS: u32 = 290;
    pub const CANCEL_INTERACTION_CHAIN: u32 = 291;
    pub const PLAY_INTERACTION_FOR: u32 = 292;
    pub const ENTITY_UPDATES: u32 = 161;
    pub const UPDATE_BLOCK_DAMAGE: u32 = 144;
    pub const DROP_CREATIVE_ITEM: u32 = 172;
    pub const DROP_ITEM_STACK: u32 = 174;
    pub const MOVE_ITEM_STACK: u32 = 175;
    pub const SMART_MOVE_ITEM_STACK: u32 = 176;
    pub const INVENTORY_ACTION: u32 = 179;
    pub const SET_CREATIVE_ITEM: u32 = 171;
    pub const SMART_GIVE_CREATIVE_ITEM: u32 = 173;
    pub const SET_ACTIVE_SLOT: u32 = 177;
    pub const SYNC_PLAYER_PREFERENCES: u32 = 116;
    pub const UPDATE_PLAYER_INVENTORY: u32 = 170;
    pub const SERVER_SET_BLOCKS: u32 = 141;
    pub const SET_CHUNK: u32 = 131;
    pub const SET_CHUNK_HEIGHTMAP: u32 = 132;
    pub const SPAWN_PARTICLE_SYSTEM: u32 = 152;
    pub const SPAWN_BLOCK_PARTICLE_SYSTEM: u32 = 153;
    pub const UPDATE_BLOCK_DAMAGES: u32 = 117;
    pub const SERVER_SET_HEIGHT: u32 = 183;
    pub const SERVER_SET_HEIGHTS: u32 = 184;
    pub const NOTIFICATION: u32 = 212;
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

/// Network ids of dropped items start here, far from players'.
const FIRST_ITEM_ID: i32 = 1_000_000;

/// Hotbar slots.

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
    /// Queued crafts (spec 09).
    queue: crate::crafting::Queue,
    /// Ids for windows the server opens start at 1.
    next_window: i32,
    /// Chains that may use up the held item: chain id → (item, hotbar slot).
    consuming: HashMap<i32, (String, i32)>,
    /// Inventory sections changed this tick (sent at its end).
    inv_dirty: std::collections::BTreeSet<i32>,
    /// The performance panel (spec 13): wanted, and already on screen.
    perf_hud: bool,
    perf_shown: bool,
    /// Columns this player's client has.
    columns: std::collections::HashSet<(i32, i32)>,
    /// Where picked-up items go first (`SyncPlayerPreferences`).
    prefs: inventory::Preferences,
    /// Active utility and tools slots (-1: none).
    utility_slot: i32,
    tools_slot: i32,
    /// The inventory and active slots as last written to the save.
    saved_carry: Option<(String, [i32; 3])>,
    /// Its stats (spec 17) and what regeneration asks about it.
    stats: crate::stats::StatMap,
    /// Its item modifiers must be worked out again (equipment changed).
    mods_dirty: bool,
    /// Stats to empty at the next recalculation (a newly held weapon's).
    stats_to_clear: Vec<i32>,
    /// The hotbar slot its last interaction chain named.
    hotbar_slot: i32,
    /// The held and utility items at the last recalculation.
    last_held: (Option<String>, Option<String>),
    /// What clients were last told it wears and holds (armor, right hand,
    /// left hand), and an update waiting for the next tick.
    equipment: Option<(Vec<String>, String, String)>,
    equipment_pending: Option<proto::ComponentUpdate>,
    sprinting: bool,
    gliding: bool,
    /// Creative settings from `SyncPlayerPreferences`: reach and no
    /// physics.
    creative_distance: i32,
    no_physics: bool,
    /// Placements already done: (chain, fork path, operation).
    placed_ops: std::collections::HashSet<(i32, String, i32)>,
    /// The interaction chains it runs (spec 18).
    chains: crate::chains::Manager,
    /// The effects active on it (spec 19).
    effects: crate::effects::Controller,
    /// The movement states the client last reported.
    movement: proto::MovementStates,
}

/// A block placement a client reports in an interaction chain (spec 15).
#[derive(Clone, Debug)]
struct Placement {
    pos: Pos,
    rot: i32,
    /// The block the client put there (-1: none named).
    client_block: i32,
    slot: i32,
}

/// The item facts inventory moves need (spec 14), from the item registry,
/// the block rules and the open window.
struct ItemsView<'a> {
    book: &'a crate::crafting::Book,
    rules: &'a crate::support::Rules,
    bench: Option<&'a crate::crafting::Bench>,
    /// An open processing bench: its asset and recipes (spec 16).
    processing: Option<(
        &'a crate::processing::Config,
        &'a [crate::processing::Recipe],
    )>,
    /// An open container: every slot takes and gives anything.
    container: bool,
}

/// The item facts processing needs (spec 16).
struct ProcItems<'a> {
    book: &'a crate::crafting::Book,
    keys: &'a HashMap<i32, String>,
}

impl crate::processing::Items for ProcItems<'_> {
    fn max_stack(&self, item: &str) -> i32 {
        self.book.max_stack(item)
    }
    fn units(&self, m: &crate::processing::Material, item: &str) -> Option<i32> {
        self.book.processing_units(m, item)
    }
    fn resource(&self, item: &str, resource: &str) -> Option<i32> {
        self.book.resource_units(item, resource)
    }
    fn fuel_quality(&self, item: &str) -> f64 {
        self.book.fuel_quality(item)
    }
    fn block_key(&self, item: &str) -> Option<String> {
        self.keys.get(&self.book.block_of(item)).cloned()
    }
    fn stack(&self, item: &str, n: i32) -> Stack {
        self.book.stack(item, n)
    }
}

impl inventory::Items for ItemsView<'_> {
    fn sort_key(&self, item: &str) -> (u8, String) {
        (self.book.sort_rank(item), self.book.item_name(item))
    }
    fn max_stack(&self, item: &str) -> i32 {
        self.book.max_stack(item)
    }
    fn armor_slot(&self, item: &str) -> Option<i32> {
        self.book.armor_slot(item)
    }
    fn usable(&self, item: &str) -> bool {
        self.book.usable(item)
    }
    fn ability(&self, item: &str) -> Option<inventory::Ability> {
        self.book.ability(item)
    }
    fn kind(&self, item: &str) -> inventory::Kind {
        use inventory::Kind;
        if self.book.armor_slot(item).is_some() {
            Kind::Armor
        } else if self.book.weapon_or_tool(item) {
            Kind::WeaponOrTool
        } else if self.book.usable(item) {
            Kind::Usable
        } else if self
            .rules
            .block(self.book.block_of(item))
            .is_some_and(|b| b.solid)
        {
            Kind::SolidBlock
        } else {
            Kind::Misc
        }
    }
    fn window_add(&self, slot: i32, item: &str) -> bool {
        if self.container {
            return true;
        }
        if let Some((cfg, recipes)) = self.processing {
            let pi = ProcItems {
                book: self.book,
                keys: &HashMap::new(),
            };
            let b = crate::processing::Bench {
                cfg,
                recipes,
                tier: 1,
                items: &pi,
            };
            let fuel = cfg.fuel.as_ref().map_or(0, Vec::len) as i32;
            let input = cfg.inputs(1).len() as i32;
            return if slot < fuel {
                b.fuel_takes(slot as usize, item)
            } else if slot < fuel + input {
                b.input_takes((slot - fuel) as usize, item)
            } else {
                // Output slots: only taken from.
                false
            };
        }
        slot == 0
            && self
                .bench
                .is_some_and(|b| !self.book.structural_options(b, item).0.is_empty())
    }
    fn window_remove(&self, slot: i32) -> bool {
        self.container || self.processing.is_some() || slot == 0
    }
    fn output_start(&self) -> i32 {
        self.processing.map_or(0, |(cfg, _)| {
            cfg.fuel.as_ref().map_or(0, Vec::len) as i32 + cfg.inputs(1).len() as i32
        })
    }
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
    /// The editor tools every player joins with (`BT item` lines).
    tools: Vec<String>,
    /// Items used up by an interaction: (item, interaction type) → (charge
    /// time, quantity change, Adventure only).
    consume: HashMap<(String, i32), (f32, i32, bool)>,
    /// The ground under the changes: the generator, or flat layers.
    ground: Mutex<Option<(Vec<i32>, Option<crate::terrain::Generator>)>>,
    /// Block support rules and drops (spec 10).
    rules: crate::support::Rules,
    /// Eye height of the player model (picked-up items fly to a fifth of it).
    eye_height: f64,
    /// When the last world tick ran.
    last_tick: Mutex<Option<std::time::Instant>>,
    /// New players see the performance panel (`--perf-hud`).
    perf_hud: bool,
    /// New players start in creative (`--game-mode creative`; adventure by
    /// default, as the official server).
    creative: bool,
    /// The stat types (spec 17).
    stat_types: crate::stats::Types,
    /// Interactions and root interactions as the server runs them.
    chains: crate::chains::Book,
    effects: crate::effects::Book,
}

/// Who gets a player's component update.
#[derive(Clone, Copy, PartialEq)]
enum Audience {
    Own,
    Others,
    All,
}

/// What stat regeneration asks about a player.
struct StatWho {
    creative: bool,
    sprinting: bool,
    gliding: bool,
}

impl crate::stats::Who for StatWho {
    fn creative(&self) -> bool {
        self.creative
    }
    fn sprinting(&self) -> bool {
        self.sprinting
    }
    fn gliding(&self) -> bool {
        self.gliding
    }
}

/// A position.
type Pos = (i32, i32, i32);

/// How a block change happens (the official server's set-block settings
/// that matter here).
#[derive(Clone, Copy, Default)]
pub struct How {
    /// Mark the block's area for support checks (a player's or physics'
    /// change).
    pub physics: bool,
    /// Removed by physics: its particle is `Physics`, not `Break`.
    pub by_physics: bool,
    /// No particles (an interaction state change).
    pub quiet: bool,
}

#[derive(Default)]
struct Inner {
    /// Particles of placements, sent after the tick's block changes and
    /// before its heights (as the official server's command buffer does).
    deferred_particles: Vec<Packet>,
    /// This tick's component updates of players: (player, who sees it,
    /// the update).
    stat_updates: Vec<(i32, Audience, proto::ComponentUpdate)>,
    /// Processing bench blocks (spec 16), by main cell.
    processing: BTreeMap<Pos, crate::processing::State>,
    /// What blocks that hold items hold (spec 21).
    containers: BTreeMap<Pos, Vec<inventory::Slot>>,
    /// Block state read from the world save, until its block is first
    /// ticked (spec 20).
    saved_states: BTreeMap<Pos, String>,
    /// The block state as last written to the save.
    states_written: String,
    next_id: i32,
    players: BTreeMap<i32, Entry>,
    /// Blocks changed since the server started, sent to whoever joins.
    placed: BTreeMap<(i32, i32, i32), i32>,
    /// Rotation index of changed blocks (absent: 0).
    rotations: HashMap<Pos, i32>,
    /// Parts of multi-block blocks: packed offset from the main cell.
    fillers: HashMap<Pos, i32>,
    /// Support values (spec 10; absent: 0, 15 decoration).
    support: HashMap<Pos, u8>,
    /// Blocks to check for support at the next tick: (section, index).
    ticking: std::collections::BTreeSet<(Pos, i32)>,
    /// Changed positions per section, sent at the end of each tick.
    sections: HashMap<Pos, crate::changes::SectionChanges>,
    /// Sections changed this tick.
    touched: std::collections::BTreeSet<Pos>,
    /// Highest non-transparent block per column, once known.
    tops: HashMap<(i32, i32), i32>,
    /// Columns whose height changed this tick: chunk column → indexes.
    heights: BTreeMap<(i32, i32), std::collections::BTreeSet<i32>>,
    /// World ticks so far.
    tick: u64,
    /// Damaged blocks: health left (1 is whole) and the last hit.
    health: HashMap<(i32, i32, i32), (f32, std::time::Instant)>,
    /// Health changes per section waiting for the end of the tick.
    damage_deltas: HashMap<Pos, crate::changes::Deltas>,
    /// Items lying on the ground, by network id.
    items: BTreeMap<i32, crate::items::Item>,
    /// Picked-up copies flying to players.
    flights: BTreeMap<i32, crate::items::Flight>,
    /// Blocks on their way down (spec 24), by network id.
    falling: BTreeMap<i32, crate::falling::Falling>,
    /// The falling blocks that moved this tick.
    falling_moved: Vec<i32>,
    /// Item entities removed since clients last heard.
    gone: Vec<i32>,
    /// Inside a world tick (items made now start moving the next tick).
    in_tick: bool,
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

    /// The `tools` table (aerie-assets): `BT item`, in slot order.
    pub fn with_tools(mut self, text: &str) -> Self {
        self.tools = text
            .lines()
            .filter_map(|l| l.strip_prefix("BT "))
            .map(|i| i.trim().to_owned())
            .collect();
        self
    }

    fn tool_stacks(&self) -> Vec<crate::inventory::Stack> {
        self.tools.iter().map(|i| self.book.stack(i, 1)).collect()
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
            if let Some(held) = inv.get(inventory::HOTBAR, slot)
                && held.item == item
            {
                inv.set(inventory::HOTBAR, slot, Some(held.with_n(held.n + adjust)));
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

    /// One hit on a block (spec 12): creative breaks it at once without
    /// drops; otherwise the spec the held item gives decides the damage,
    /// effects and drops.
    fn hit(&self, player: i32, x: i32, y: i32, z: i32, slot: i32) {
        self.world_step(|hub, g, ground, out| {
            hub.hit_in(g, ground, out, player, (x, y, z), slot, None)
        });
    }

    /// [`Hub::hit`] inside a world step. `tool` is the break tool and
    /// match rule of the break operation, when a chain names them.
    #[allow(clippy::too_many_arguments)]
    fn hit_in(
        &self,
        g: &mut Inner,
        ground: &Ground,
        out: &mut Outgoing,
        player: i32,
        at: Pos,
        slot: i32,
        tool: Option<(Option<String>, bool)>,
    ) {
        // A tool with a break shape hits every block of the shape around
        // the one aimed at (spec 12, "Break shapes").
        let shape = g
            .players
            .get(&player)
            .and_then(|e| e.inv.get(inventory::HOTBAR, slot))
            .and_then(|st| self.gather.item(&st.item).and_then(|i| i.shape));
        let Some(shape) = shape else {
            self.hit_one(g, ground, out, player, at, slot, tool, true);
            return;
        };
        let Some(e) = g.players.get(&player) else {
            return;
        };
        let (yaw, pitch) = e
            .look
            .as_ref()
            .map_or((0.0, -90f32.to_radians()), |l| (l.yaw, l.pitch));
        let dir = {
            let len = pitch.cos();
            [
                (len * -yaw.sin()) as f64,
                pitch.sin() as f64,
                (len * -yaw.cos()) as f64,
            ]
        };
        let anchor = [at.0, at.1, at.2];
        let basis = match (&e.position, shape.surface) {
            (Some(p), true) => {
                crate::gather::surface_basis([p.x, p.y + self.eye_height, p.z], dir, anchor)
            }
            _ => crate::gather::view_basis(dir),
        };
        let creative = e.creative;
        let stack = e.inv.get(inventory::HOTBAR, slot);
        let anchor_block = {
            use crate::support::World;
            View {
                g,
                ground,
                hub_rules: &self.rules,
            }
            .block(at.0, at.1, at.2)
            .0
        };
        for c in crate::gather::shape_cells(&shape, anchor, basis) {
            if !(0..crate::support::WORLD_HEIGHT).contains(&c[1]) {
                continue;
            }
            self.hit_one(
                g,
                ground,
                out,
                player,
                (c[0], c[1], c[2]),
                slot,
                tool.clone(),
                shape.per_block,
            );
        }
        // Once per swing: the wear of the block first aimed at.
        if !creative
            && !shape.per_block
            && let Some(st) = stack.filter(|s| !s.unbreakable())
            && let Some(key) = self.keys_by_id.get(&anchor_block).cloned()
        {
            self.wear_held(g, player, slot, &st, &key, anchor_block, out);
        }
    }

    /// One block taking a hit (spec 12); `wear` whether the held tool
    /// wears for it.
    #[allow(clippy::too_many_arguments)]
    fn hit_one(
        &self,
        g: &mut Inner,
        ground: &Ground,
        out: &mut Outgoing,
        player: i32,
        (x, y, z): Pos,
        slot: i32,
        tool: Option<(Option<String>, bool)>,
        wear: bool,
    ) {
        let hub = self;
        {
            use crate::support::World;
            let creative = g.players.get(&player).is_none_or(|e| e.creative);
            // The server's own idea of the held stack, as the official
            // server uses (not the id the client names).
            let stack = g
                .players
                .get(&player)
                .and_then(|e| e.inv.get(inventory::HOTBAR, slot))
                .filter(|s| !s.item.is_empty() && s.item != "Empty");
            let held = stack.as_ref().map(|s| s.item.clone());
            let broken = stack.as_ref().is_some_and(Stack::broken);
            // A part of a multi-block block: the hit goes to its main cell.
            let f = unpack_filler(g.fillers.get(&(x, y, z)).copied().unwrap_or(0));
            let (x, y, z) = (x - f[0], y - f[1], z - f[2]);
            let (block, rot) = View {
                g,
                ground,
                hub_rules: &hub.rules,
            }
            .block(x, y, z);
            if block == 0 {
                return;
            }
            let center = hub.block_center(block, rot, (x, y, z));
            if creative {
                hub.break_block(g, ground, (x, y, z), None, out);
                return;
            }
            let Some(key) = hub.keys_by_id.get(&block).cloned() else {
                return;
            };
            let deco = g.support.get(&(x, y, z)) == Some(&crate::support::DECO);
            // The break tool the held item's primary interaction runs.
            let (tool, match_tool) =
                tool.unwrap_or_else(|| hub.gather.break_tool(held.as_deref(), "Primary"));
            match hub.gather.hit_with(
                &key,
                held.as_deref(),
                deco,
                broken,
                tool.as_deref(),
                match_tool,
            ) {
                crate::gather::Hit::Nothing => {}
                crate::gather::Hit::Unbreakable => {
                    hub.effect(g, &hub.gather.unbreakable.clone(), center, out);
                    if let Some(i) = held.as_deref().and_then(|h| hub.gather.item(h))
                        && i.tool
                        && i.incorrect_sound > 0
                    {
                        hub.sound_near(g, i.incorrect_sound, center, out);
                    }
                }
                crate::gather::Hit::Damage {
                    damage,
                    item,
                    list,
                    quantity,
                    spec,
                    tool,
                } => {
                    let (sec, index) = section_of(x, y, z);
                    let now = std::time::Instant::now();
                    let h = g.health.get(&(x, y, z)).map_or(1.0, |h| h.0) - damage;
                    let destroyed = h < 0.0 || h.abs() < 1.0e-5;
                    if h > 0.0 && h < 1.0 && !destroyed {
                        g.health.insert((x, y, z), (h, now));
                        let d = g.damage_deltas.entry(sec).or_default();
                        d.add(index, -damage);
                    } else {
                        g.health.remove(&(x, y, z));
                        if let Some(d) = g.damage_deltas.get_mut(&sec) {
                            d.remove(index);
                        }
                    }
                    if destroyed {
                        match tool {
                            None => {
                                let drops = (item, list, quantity);
                                hub.break_block(g, ground, (x, y, z), Some(drops), out);
                            }
                            Some(t) => hub.tool_break(g, ground, (x, y, z), block, rot, t, out),
                        }
                    } else if let Some(b) = hub.rules.block(block)
                        && b.hit_sound > 0
                    {
                        hub.sound_near(g, b.hit_sound, center, out);
                    }
                    let layer = spec.as_ref().map_or(0, |s| s.hit_sound).max(0);
                    let layer = if layer != 0 {
                        layer
                    } else {
                        held.as_deref()
                            .and_then(|h| hub.gather.item(h))
                            .filter(|i| i.tool)
                            .map_or(0, |i| i.hit_sound)
                    };
                    if layer != 0 {
                        hub.sound_near(g, layer, center, out);
                    }
                    if spec.as_ref().is_some_and(|s| s.incorrect) {
                        hub.effect(g, &hub.gather.incorrect.clone(), center, out);
                    }
                    if wear && let Some(st) = stack.filter(|s| !s.unbreakable()) {
                        hub.wear_held(g, player, slot, &st, &key, block, out);
                    }
                }
            }
        }
    }

    /// The held tool's wear after a damaging hit (spec 12): the slot is
    /// replaced (and sent, even unchanged, as the official replace is), and
    /// a tool that just broke tells its player.
    #[allow(clippy::too_many_arguments)]
    fn wear_held(
        &self,
        g: &mut Inner,
        player: i32,
        slot: i32,
        st: &Stack,
        key: &str,
        block: i32,
        out: &mut Outgoing,
    ) {
        let loss = self
            .gather
            .wear(&st.item, key, |s| self.rules.in_set(s, block));
        let worn = st.worn(-loss);
        let Some(e) = g.players.get_mut(&player) else {
            return;
        };
        if !e
            .inv
            .get(inventory::HOTBAR, slot)
            .is_some_and(|s| s.stacks_with(st))
        {
            return;
        }
        e.inv.set(inventory::HOTBAR, slot, Some(worn.clone()));
        e.inv_dirty.insert(inventory::HOTBAR);
        if worn.broken() && !st.broken() {
            self.tell_broken(player, st, out);
        }
    }

    /// An item just broke: the chat line and the sound its owner gets.
    fn tell_broken(&self, player: i32, st: &Stack, out: &mut Outgoing) {
        {
            let name = proto::FormattedMessage {
                message_id: Some(self.book.item_name(&st.item)),
                ..Default::default()
            };
            let msg = proto::FormattedMessage {
                message_id: Some("server.general.repair.itemBroken".into()),
                message_params: Some(vec![("itemName".into(), name)]),
                color: Some("#ff5555".into()),
                ..Default::default()
            };
            if let Ok(p) = packet(
                id::SERVER_MESSAGE,
                &proto::ServerMessage {
                    r#type: proto::ChatType::CHAT,
                    message: Some(msg),
                },
            ) {
                out.to.push((player, p));
            }
            let s = self.book.sound("SFX_Item_Break");
            if s > 0
                && let Ok(p) = sound_2d_packet(s)
            {
                out.to.push((player, p));
            }
        }
    }

    /// The centre of a block (its hitbox's centre).
    fn block_center(&self, id: i32, rot: i32, (x, y, z): Pos) -> [f64; 3] {
        match self.rules.center(id, rot) {
            Some(c) => [x as f64 + c[0], y as f64 + c[1], z as f64 + c[2]],
            None => [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5],
        }
    }

    /// A block broken by a player (spec 12): break sound, the block gone
    /// with its area marked for support checks, then the drops (none in
    /// creative).
    fn break_block(
        &self,
        g: &mut Inner,
        ground: &Ground,
        pos: Pos,
        drops: Option<(Option<String>, Option<String>, i32)>,
        out: &mut Outgoing,
    ) {
        self.break_with(g, ground, pos, drops, false, true, out);
    }

    /// Breaking with the official settings: `by_physics` (its particle is
    /// `Physics`), `area` (support checks around it).
    #[allow(clippy::too_many_arguments)]
    fn break_with(
        &self,
        g: &mut Inner,
        ground: &Ground,
        (x, y, z): Pos,
        drops: Option<(Option<String>, Option<String>, i32)>,
        by_physics: bool,
        area: bool,
        out: &mut Outgoing,
    ) {
        use crate::support::World;
        let (block, rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(x, y, z);
        let center = self.block_center(block, rot, (x, y, z));
        if let Some(b) = self.rules.block(block)
            && b.break_sound > 0
        {
            self.sound_at(g, b.break_sound, center, true, out);
        }
        let how = How {
            physics: area,
            by_physics,
            ..Default::default()
        };
        if self.change(g, ground, (x, y, z), 0, 0, how, out).is_some() && area {
            self.mark_area(g, x, y, z, block, rot);
        }
        if !by_physics {
            self.connect(g, ground, (x, y, z), false, out);
        }
        let Some((item, list, quantity)) = drops else {
            return;
        };
        if quantity <= 0 {
            return;
        }
        let stacks = if item.is_none() && list.is_none() {
            self.rules
                .block(block)
                .and_then(|b| b.item.clone())
                .map(|i| vec![(i, quantity)])
                .unwrap_or_default()
        } else {
            self.rules.roll(&crate::support::Drop {
                item,
                list,
                quantity,
            })
        };
        out.drops
            .push((stacks, [x as f64 + 0.5, y as f64, z as f64 + 0.5]));
    }

    /// A block finished by a tool it has an entry for (spec 12): it turns
    /// into the entry's state if that is another block (rotation 0, the
    /// old block's break sound), else breaks without its usual drops; the
    /// entry's drops (or the block's own item) fall either way.
    #[allow(clippy::too_many_arguments)]
    fn tool_break(
        &self,
        g: &mut Inner,
        ground: &Ground,
        (x, y, z): Pos,
        block: i32,
        rot: i32,
        t: crate::gather::ToolData,
        out: &mut Outgoing,
    ) {
        let state = t.state.as_ref().and_then(|s| {
            self.rules
                .block(block)?
                .states
                .iter()
                .find(|(n, _)| n == s)
                .map(|(_, id)| *id)
        });
        match state.filter(|&n| n != block) {
            Some(n) => {
                self.change(g, ground, (x, y, z), n, 0, How::default(), out);
                if let Some(b) = self.rules.block(block)
                    && b.break_sound > 0
                {
                    let center = self.block_center(block, rot, (x, y, z));
                    self.sound_near(g, b.break_sound, center, out);
                }
            }
            None => self.break_block(g, ground, (x, y, z), None, out),
        }
        let stacks = if t.item.is_none() && t.list.is_none() {
            self.rules
                .block(block)
                .and_then(|b| b.item.clone())
                .map(|i| vec![(i, 1)])
                .unwrap_or_default()
        } else {
            self.rules.roll(&crate::support::Drop {
                item: t.item,
                list: t.list,
                quantity: 1,
            })
        };
        if !stacks.is_empty() {
            out.drops
                .push((stacks, [x as f64 + 0.5, y as f64, z as f64 + 0.5]));
        }
    }

    /// A sound at a point for the players within its reach (SFX).
    fn sound_near(&self, g: &Inner, index: i32, at: [f64; 3], out: &mut Outgoing) {
        self.sound_at(g, index, at, false, out);
    }

    /// The same, `placed`: kept where it was made among the packets for
    /// everyone (a block's break sound comes before its particle).
    fn sound_at(&self, g: &Inner, index: i32, at: [f64; 3], placed: bool, out: &mut Outgoing) {
        let reach = self.rules.sound_distance(index) as f64;
        let Ok(p) = sound_3d_packet(index, proto::SoundCategory::SFX, at) else {
            return;
        };
        for (&pid, e) in &g.players {
            if let Some(q) = &e.position
                && dist2([q.x, q.y, q.z], at) <= reach * reach
            {
                if placed {
                    out.placed.push((out.all.len(), pid, p.clone()));
                } else {
                    out.to.push((pid, p.clone()));
                }
            }
        }
    }

    /// A gathering effect: its particle system for players within 75
    /// blocks, its sound.
    fn effect(&self, g: &Inner, e: &crate::gather::Effect, at: [f64; 3], out: &mut Outgoing) {
        if let Some(name) = &e.particle
            && let Ok(p) = packet(
                id::SPAWN_PARTICLE_SYSTEM,
                &proto::SpawnParticleSystem {
                    particle_system_id: Some(name.clone()),
                    position: Some(proto::Position {
                        x: at[0],
                        y: at[1],
                        z: at[2],
                    }),
                    rotation: None,
                    scale: 1.0,
                    color: None,
                    max_duration: 0.0,
                },
            )
        {
            for (&pid, pe) in &g.players {
                if let Some(q) = &pe.position
                    && dist2([q.x, q.y, q.z], at) <= 75.0 * 75.0
                {
                    out.to.push((pid, p.clone()));
                }
            }
        }
        if e.sound > 0 {
            self.sound_near(g, e.sound, at, out);
        }
    }

    /// Block health for one tick (spec 12): blocks left alone for 5 s heal
    /// 0.1 a second; the changes go out per section.
    fn health_step(&self, g: &mut Inner, dt: f32, out: &mut Outgoing) {
        let now = std::time::Instant::now();
        let healing: Vec<(Pos, f32)> = g
            .health
            .iter()
            .filter(|(_, (_, last))| now.duration_since(*last).as_millis() >= 5000)
            .map(|(p, (h, _))| (*p, *h))
            .collect();
        for (p, h) in healing {
            let (sec, index) = section_of(p.0, p.1, p.2);
            let mut delta = 0.1 * dt;
            let after = h + delta;
            if after < 1.0 {
                if let Some(x) = g.health.get_mut(&p) {
                    x.0 = after;
                }
            } else {
                delta = 1.0 - h;
                g.health.remove(&p);
            }
            g.damage_deltas.entry(sec).or_default().add(index, delta);
        }
        let secs: Vec<Pos> = g.damage_deltas.keys().copied().collect();
        for sec in secs {
            let Some(d) = g.damage_deltas.get_mut(&sec) else {
                continue;
            };
            let list = d.take();
            if list.is_empty() {
                continue;
            }
            let health = |g: &Inner, i: i32| {
                let (x, y, z) = block_of(sec, i);
                g.health.get(&(x, y, z)).map_or(1.0, |h| h.0)
            };
            let p = if list.len() == 1 {
                let (i, delta) = list[0];
                let (x, y, z) = block_of(sec, i);
                packet(
                    id::UPDATE_BLOCK_DAMAGE,
                    &proto::UpdateBlockDamage {
                        block_position: Some(proto::BlockPosition { x, y, z }),
                        damage: health(g, i),
                        delta,
                    },
                )
            } else {
                let cmds = list
                    .iter()
                    .map(|&(i, delta)| proto::BlockDamageCmd {
                        index: i as i16,
                        damage: health(g, i),
                        delta,
                    })
                    .collect();
                packet(
                    id::UPDATE_BLOCK_DAMAGES,
                    &proto::UpdateBlockDamages {
                        x: sec.0,
                        y: sec.1,
                        z: sec.2,
                        cmds,
                    },
                )
            };
            if let Ok(p) = p {
                out.all.push(p);
            }
        }
    }

    /// Stacks dropped together at one place: straight up for one, spread
    /// on a circle for more.
    pub fn drop_items(&self, stacks: Vec<Stack>, at: [f64; 3]) {
        if stacks.is_empty() {
            return;
        }
        let angle = rand::random::<f32>() * std::f32::consts::TAU;
        let vels = crate::items::launch(stacks.len(), angle);
        let mut g = self.inner.lock().unwrap();
        let born = g.tick + g.in_tick as u64;
        for (st, vel) in stacks.into_iter().zip(vels) {
            if st.n <= 0 {
                continue;
            }
            g.next_item = g.next_item.max(FIRST_ITEM_ID) + 1;
            let nid = g.next_item;
            let life = self.item_life(&st.item);
            g.items
                .insert(nid, crate::items::Item::new(&st, at, vel, life, born));
        }
    }

    /// Seconds an item lies before it disappears (the default gameplay
    /// config's `ItemEntity.Lifetime`).
    fn item_life(&self, _item: &str) -> f32 {
        600.0
    }

    pub fn with_effects(mut self, b: crate::effects::Book) -> Self {
        self.effects = b;
        self
    }

    pub fn with_chains(mut self, b: crate::chains::Book) -> Self {
        self.chains = b;
        self
    }

    pub fn with_stats(mut self, t: crate::stats::Types) -> Self {
        self.stat_types = t;
        self
    }

    /// A player's stat values in the order of its first update (spec 17).
    pub fn stat_values(&self, id: i32) -> Option<Vec<(i32, f32)>> {
        let g = self.inner.lock().unwrap();
        let e = g.players.get(&id)?;
        Some(
            e.stats
                .init(&self.stat_types, true)
                .entity_stat_updates
                .into_iter()
                .map(|(i, u)| (i, u[0].value))
                .collect(),
        )
    }

    fn stats_file(&self, uuid: uuid::Uuid) -> Option<std::path::PathBuf> {
        Some(
            self.save
                .as_ref()?
                .with_file_name("stats")
                .join(format!("{uuid}.txt")),
        )
    }

    fn saved_stats(&self, uuid: uuid::Uuid) -> Option<String> {
        std::fs::read_to_string(self.stats_file(uuid)?).ok()
    }

    fn save_stats(&self, uuid: uuid::Uuid, stats: &crate::stats::StatMap) {
        let Some(f) = self.stats_file(uuid) else {
            return;
        };
        let r = std::fs::create_dir_all(f.parent().unwrap())
            .and_then(|_| std::fs::write(&f, stats.to_text(&self.stat_types)));
        if let Err(e) = r {
            tracing::warn!("saving stats failed: {e}");
        }
    }

    /// Stats for one tick (spec 17): regeneration, then each player's
    /// pending updates are set aside for the tick's entity updates.
    fn stats_step(&self, g: &mut Inner, dt: f32) {
        for (pid, e) in g.players.iter_mut() {
            let who = StatWho {
                creative: e.creative,
                sprinting: e.sprinting,
                gliding: e.gliding,
            };
            self.recalculate_modifiers(e);
            // Creative players are invulnerable.
            e.stats.regenerate(&self.stat_types, &who, dt, e.creative);
            if e.view.is_none() {
                continue;
            }
            let (own, others) = e.stats.take_updates();
            if let Some(u) = own {
                g.stat_updates.push((
                    *pid,
                    Audience::Own,
                    proto::ComponentUpdate::EntityStatsUpdate(Box::new(u)),
                ));
            }
            if let Some(u) = others {
                g.stat_updates.push((
                    *pid,
                    Audience::Others,
                    proto::ComponentUpdate::EntityStatsUpdate(Box::new(u)),
                ));
            }
            // What it wears and holds, shown to everyone the tick after it
            // changed (spec 17).
            if let Some(u) = e.equipment_pending.take() {
                g.stat_updates.push((*pid, Audience::All, u));
            }
            let mut armor = vec![String::new(); 4];
            for (i, a) in armor.iter_mut().enumerate() {
                if let Some(s) = e.inv.get(inventory::ARMOR, i as i32) {
                    *a = s.item;
                }
            }
            let hand = |s: Option<Stack>| s.map_or("Empty".to_owned(), |s| s.item);
            let now = (
                armor,
                hand(e.inv.get(inventory::HOTBAR, e.hotbar_slot)),
                hand(
                    (e.utility_slot >= 0)
                        .then(|| e.inv.get(inventory::UTILITY, e.utility_slot))
                        .flatten(),
                ),
            );
            if e.equipment.as_ref() != Some(&now) {
                let first = e.equipment.is_none();
                e.equipment = Some(now.clone());
                let u = proto::ComponentUpdate::EquipmentUpdate(Box::new(proto::EquipmentUpdate {
                    armor_ids: Some(now.0),
                    right_hand_item_id: Some(now.1),
                    left_hand_item_id: Some(now.2),
                }));
                // New joiners see it as it is now.
                if let Some(v) = e.view.as_mut() {
                    for c in v.updates.iter_mut().flatten() {
                        if matches!(c, proto::ComponentUpdate::EquipmentUpdate(_)) {
                            *c = u.clone();
                        }
                    }
                }
                let empty = e.equipment.as_ref().is_some_and(|q| {
                    q.0.iter().all(String::is_empty) && q.1 == "Empty" && q.2 == "Empty"
                });
                if !(first && empty) {
                    e.equipment_pending = Some(u);
                }
            }
        }
    }

    /// A player's stat modifiers from what it wears and holds (spec 17),
    /// worked out again when its equipment changed: a newly held weapon's
    /// or utility item's stats are emptied first, then the armor's summed
    /// modifiers, the held weapon's and (when the held item allows a
    /// utility item) the utility item's.
    fn recalculate_modifiers(&self, e: &mut Entry) {
        let t = &self.stat_types;
        let held = e.inv.get(inventory::HOTBAR, e.hotbar_slot);
        let utility = (e.utility_slot >= 0)
            .then(|| e.inv.get(inventory::UTILITY, e.utility_slot))
            .flatten();
        let now = (
            held.as_ref().map(|s| s.item.clone()),
            utility.as_ref().map(|s| s.item.clone()),
        );
        if now != e.last_held {
            if now.0 != e.last_held.0
                && let Some(s) = now.0.as_deref().and_then(|i| self.book.item_stats(i))
            {
                e.stats_to_clear.extend(&s.weapon_clear);
            }
            if now.1 != e.last_held.1
                && let Some(s) = now.1.as_deref().and_then(|i| self.book.item_stats(i))
            {
                e.stats_to_clear.extend(&s.utility_clear);
            }
            e.last_held = now;
            e.mods_dirty = true;
        }
        if !e.mods_dirty {
            return;
        }
        e.mods_dirty = false;
        for i in std::mem::take(&mut e.stats_to_clear) {
            e.stats
                .minimize(t, i as usize, crate::stats::Predictable::Own);
        }
        e.stats
            .apply_summed(t, "Effect", &e.effects.modifiers(&self.effects));
        let mut sums = Vec::new();
        for slot in 0..4 {
            let Some(st) = e.inv.get(inventory::ARMOR, slot) else {
                continue;
            };
            let Some(s) = self.book.item_stats(&st.item) else {
                continue;
            };
            for (stat, mods) in &s.armor {
                for m in mods {
                    let mut a = m.amount;
                    if st.broken() {
                        a *= self.gather.broken_armor_penalty as f32;
                    }
                    sums.push((*stat, m.multiplicative, a));
                }
            }
        }
        e.stats.apply_summed(t, "Armor", &sums);
        let held_stats = held.as_ref().and_then(|s| self.book.item_stats(&s.item));
        e.stats
            .apply_item(t, "*Weapon_", held_stats.and_then(|s| s.weapon.as_deref()));
        if held_stats.is_none_or(|s| s.utility_compatible) {
            let u = utility
                .as_ref()
                .and_then(|s| self.book.item_stats(&s.item))
                .and_then(|s| s.utility.as_deref());
            e.stats.apply_item(t, "*Utility_", u);
        }
    }

    /// `/player stats …` as the official command: add, set, get, reset,
    /// settomax; the reply is the official message.
    fn stats_command(&self, id: i32, args: &[&str]) -> Result<()> {
        let reply = |key: &str, name: &str, value: Option<f32>| -> Result<Packet> {
            let mut params = vec![(
                "name".to_owned(),
                proto::ParamValue::StringParamValue(Box::new(proto::StringParamValue {
                    value: Some(name.to_owned()),
                })),
            )];
            if let Some(v) = value {
                params.push((
                    "value".to_owned(),
                    proto::ParamValue::DoubleParamValue(Box::new(proto::DoubleParamValue {
                        value: v as f64,
                    })),
                ));
            }
            packet(
                id::SERVER_MESSAGE,
                &proto::ServerMessage {
                    r#type: proto::ChatType::CHAT,
                    message: Some(proto::FormattedMessage {
                        message_id: Some(key.to_owned()),
                        params: Some(params),
                        ..Default::default()
                    }),
                },
            )
        };
        let (op, name, amount) = match args {
            [op, name] => (*op, *name, None),
            [op, name, n] => (*op, *name, n.parse::<i32>().ok()),
            _ => {
                self.tell(
                    id,
                    "Usage: /player stats <add|set|get|reset|settomax> <stat> [value]",
                );
                return Ok(());
            }
        };
        let t = &self.stat_types;
        let Some(i) = t.index(name) else {
            self.send(
                id,
                reply("server.commands.entityStats.entityStatNotFound", name, None)?,
            );
            return Ok(());
        };
        use crate::stats::Predictable::None as NoPrediction;
        let answer = {
            let mut g = self.inner.lock().unwrap();
            let Some(e) = g.players.get_mut(&id) else {
                return Ok(());
            };
            match (op, amount) {
                ("add", Some(n)) => Some((
                    "server.commands.entityStats.success",
                    e.stats.add(t, i, n as f32, NoPrediction),
                )),
                ("set", Some(n)) => Some((
                    "server.commands.entityStats.success",
                    e.stats.set(t, i, n as f32, NoPrediction),
                )),
                ("settomax", _) => {
                    let max = e.stats.max(i).unwrap_or(0.0);
                    Some((
                        "server.commands.entityStats.success",
                        e.stats.set(t, i, max, NoPrediction),
                    ))
                }
                ("reset", _) => Some((
                    "server.commands.entityStats.valueReset",
                    e.stats.reset(t, i),
                )),
                ("get", _) => Some((
                    "server.commands.entityStats.value",
                    e.stats.get(i).unwrap_or(0.0),
                )),
                _ => None,
            }
        };
        match answer {
            Some((key, v)) => self.send(id, reply(key, name, Some(v))?),
            None => self.tell(
                id,
                "Usage: /player stats <add|set|get|reset|settomax> <stat> [value]",
            ),
        }
        Ok(())
    }

    pub fn with_creative(mut self, on: bool) -> Self {
        self.creative = on;
        self
    }

    pub fn default_creative(&self) -> bool {
        self.creative
    }

    pub fn with_perf_hud(mut self, on: bool) -> Self {
        self.perf_hud = on;
        self
    }

    /// Columns a player's client now has (for the loaded count).
    pub fn columns_sent(&self, id: i32, cols: impl IntoIterator<Item = (i32, i32)>) {
        if let Some(e) = self.inner.lock().unwrap().players.get_mut(&id) {
            e.columns.extend(cols);
        }
    }

    /// What the world holds, for the performance numbers: columns some
    /// client has, entities (players, items, flying pickups), players.
    pub fn perf_counts(&self) -> crate::perf::Counts {
        let g = self.inner.lock().unwrap();
        let mut cols: std::collections::HashSet<(i32, i32)> = Default::default();
        for e in g.players.values() {
            cols.extend(e.columns.iter().copied());
        }
        crate::perf::Counts {
            columns: cols.len(),
            entities: g.players.len() + g.items.len() + g.flights.len(),
            players: g.players.len(),
        }
    }

    /// Sends the second's numbers to the players who have the panel on.
    pub fn perf_panel(&self, s: &crate::perf::Sample) {
        let mut g = self.inner.lock().unwrap();
        for e in g.players.values_mut() {
            if !e.perf_hud || e.position.is_none() {
                continue;
            }
            let hud = if e.perf_shown {
                crate::perf::update(s)
            } else {
                crate::perf::show(s)
            };
            if let Ok(p) = packet(id::CUSTOM_HUD, &hud) {
                let _ = e.outbox.send(p);
                e.perf_shown = true;
            }
        }
    }

    /// `/perf`: the panel on or off for one player.
    fn toggle_perf(&self, id: i32) -> bool {
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.players.get_mut(&id) else {
            return false;
        };
        e.perf_hud = !e.perf_hud;
        if !e.perf_hud && e.perf_shown {
            e.perf_shown = false;
            if let Ok(p) = packet(id::CUSTOM_HUD, &crate::perf::hide()) {
                let _ = e.outbox.send(p);
            }
        }
        e.perf_hud
    }

    pub fn with_eye_height(mut self, h: f64) -> Self {
        self.eye_height = h;
        self
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
            let slots = self.saved_slots(uuid).unwrap_or([0, -1, -1]);
            g.players.insert(
                id,
                Entry {
                    name: name.to_owned(),
                    uuid,
                    outbox: tx,
                    position: None,
                    teleport_id: 0,
                    inv: {
                        let mut inv = self.saved_inventory(uuid).unwrap_or_default();
                        inv.set_tools(&self.tool_stacks());
                        inv
                    },
                    creative: self.creative,
                    view: None,
                    body: None,
                    look: None,
                    window: None,
                    queue: Default::default(),
                    next_window: 0,
                    consuming: HashMap::new(),
                    perf_hud: self.perf_hud,
                    perf_shown: false,
                    columns: Default::default(),
                    prefs: Default::default(),
                    utility_slot: slots[1],
                    tools_slot: slots[2],
                    saved_carry: None,
                    stats: {
                        let mut m = crate::stats::StatMap::new(&self.stat_types);
                        if let Some(t) = self.saved_stats(uuid) {
                            m.load(&self.stat_types, &t);
                        }
                        m
                    },
                    mods_dirty: true,
                    stats_to_clear: Vec::new(),
                    hotbar_slot: slots[0],
                    last_held: (None, None),
                    equipment: None,
                    equipment_pending: None,
                    sprinting: false,
                    gliding: false,
                    creative_distance: 10,
                    no_physics: false,
                    placed_ops: Default::default(),
                    chains: Default::default(),
                    effects: Default::default(),
                    movement: Default::default(),
                    inv_dirty: Default::default(),
                },
            );
            id
        };
        self.broadcast_text(&format!("{name} joined"));
        (id, rx)
    }

    /// How many players are in the world.
    pub fn player_count(&self) -> usize {
        self.inner.lock().unwrap().players.len()
    }

    /// Queues the same packet (a disconnect) for every player.
    pub fn disconnect_all(&self, (pid, payload): &Packet) {
        let g = self.inner.lock().unwrap();
        for e in g.players.values() {
            let _ = e.outbox.send((*pid, payload.clone()));
        }
    }

    /// Saves what players carry and where they are, and the block state,
    /// with the same code paths as a player leaving (spec 25).
    pub fn save_all(&self) {
        let mut g = self.inner.lock().unwrap();
        for e in g.players.values() {
            self.save_inventory(e.uuid, &e.inv);
            self.save_slots(e.uuid, [e.hotbar_slot, e.utility_slot, e.tools_slot]);
            self.save_stats(e.uuid, &e.stats);
            if let Some(p) = &e.position {
                self.save_position(e.uuid, p);
            }
        }
        self.save_states(&mut g);
    }

    pub fn leave(&self, id: i32) {
        let gone = self.inner.lock().unwrap().players.remove(&id);
        if let Some(e) = gone {
            self.save_inventory(e.uuid, &e.inv);
            self.save_slots(e.uuid, [e.hotbar_slot, e.utility_slot, e.tools_slot]);
            self.save_stats(e.uuid, &e.stats);
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
        // Only items that exist: a stack of anything else jams the
        // chains that name the held item.
        if !self.book.known(item) {
            return Ok(0);
        }
        let slot = self.with_inventory(id, |inv| {
            let slot = (0..9)
                .find(|&i| {
                    inv.get(inventory::HOTBAR, i)
                        .is_some_and(|x| x.item == item)
                })
                .or_else(|| (0..9).find(|&i| inv.get(inventory::HOTBAR, i).is_none()))
                .unwrap_or(8);
            inv.set(inventory::HOTBAR, slot, Some(self.book.stack(item, qty)));
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
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.players.get_mut(&id) else {
            return Ok(None);
        };
        let before = e.inv.clone();
        let r = f(&mut e.inv);
        let changed = e.inv.changed_since(&before);
        e.inv_dirty.extend(changed);
        Ok(Some(r))
    }

    /// Inventory packets from the client: moving stacks (`MoveItemStack`,
    /// `SmartMoveItemStack`) and dropping them (`DropItemStack`,
    /// `DropCreativeItem`) in front of the player.
    pub fn inventory_action(&self, id: i32, packet_id: u32, payload: &[u8]) -> Result<()> {
        match packet_id {
            self::id::MOVE_ITEM_STACK => {
                let (m, _) = proto::MoveItemStack::decode(payload, 0)?;
                if self.swap_move(id, &m) {
                    return Ok(());
                }
                self.inventory_move(id, |inv, items, _, _| {
                    inv.move_to_slot(
                        items,
                        (m.from_section_id, m.from_slot_id),
                        m.quantity,
                        (m.to_section_id, m.to_slot_id),
                    );
                });
                // Moved into the utility section: that slot becomes the
                // active one, and the client is told.
                if m.to_section_id != m.from_section_id && m.to_section_id == inventory::UTILITY {
                    let changed = {
                        let mut g = self.inner.lock().unwrap();
                        g.players.get_mut(&id).is_some_and(|e| {
                            let c = e.utility_slot != m.to_slot_id;
                            e.utility_slot = m.to_slot_id;
                            c
                        })
                    };
                    if changed {
                        self.send(
                            id,
                            packet(
                                self::id::SET_ACTIVE_SLOT,
                                &proto::SetActiveSlot {
                                    inventory_section_id: inventory::UTILITY,
                                    active_slot: m.to_slot_id,
                                },
                            )?,
                        );
                    }
                }
            }
            self::id::SMART_MOVE_ITEM_STACK => {
                let (m, _) = proto::SmartMoveItemStack::decode(payload, 0)?;
                self.inventory_move(id, |inv, items, prefs, window| {
                    inv.smart_move(
                        items,
                        prefs,
                        window,
                        (m.from_section_id, m.from_slot_id),
                        m.quantity,
                        m.move_type.0,
                    );
                });
            }
            self::id::INVENTORY_ACTION => {
                let (m, _) = proto::InventoryAction::decode(payload, 0)?;
                self.inventory_move(id, |inv, items, prefs, window| {
                    inv.action(
                        items,
                        prefs,
                        window,
                        m.inventory_section_id,
                        m.inventory_action_type.0 as i32,
                    );
                });
            }
            self::id::DROP_ITEM_STACK => {
                let (m, _) = proto::DropItemStack::decode(payload, 0)?;
                // All of the quantity or nothing, as the official remove.
                if let Some(Some(st)) = self.with_inventory(id, |inv| {
                    let have = inv
                        .get(m.inventory_section_id, m.slot_id)
                        .map_or(0, |s| s.n);
                    if m.quantity > have {
                        return None;
                    }
                    inv.take(m.inventory_section_id, m.slot_id, m.quantity)
                })? {
                    // Thrown along the look at speed 6, with the drop sound
                    // (spec 11).
                    self.throw_item(id, st, 6.0);
                    self.sound_2d(id, self.book.sound("SFX_Player_Drop_Item"))?;
                }
            }
            self::id::DROP_CREATIVE_ITEM => {
                let (m, _) = proto::DropCreativeItem::decode(payload, 0)?;
                let creative = self
                    .inner
                    .lock()
                    .unwrap()
                    .players
                    .get(&id)
                    .is_some_and(|e| e.creative);
                if creative {
                    let st = self.book.stack(&m.item.item_id, m.item.quantity);
                    self.throw_item(id, st, 1.0);
                } else {
                    let n = proto::Notification {
                        message: Some(proto::FormattedMessage {
                            message_id: Some(
                                "server.general.setCreativeItem.notInCreativeMode".into(),
                            ),
                            ..Default::default()
                        }),
                        ..Default::default()
                    };
                    self.send(id, packet(self::id::NOTIFICATION, &n)?);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Runs an inventory move with the open structural window's slots
    /// joined as a section (spec 14), then puts them back: a new input
    /// remakes the options.
    fn inventory_move(
        &self,
        id: i32,
        f: impl FnOnce(&mut Inventory, &ItemsView, &inventory::Preferences, Option<i32>),
    ) {
        let mut g = self.inner.lock().unwrap();
        self.inventory_move_in(&mut g, id, f);
    }

    /// [`Hub::inventory_move`] with the world already in hand.
    fn inventory_move_in(
        &self,
        g: &mut Inner,
        id: i32,
        f: impl FnOnce(&mut Inventory, &ItemsView, &inventory::Preferences, Option<i32>),
    ) {
        let Inner {
            players,
            processing,
            containers,
            ..
        } = &mut *g;
        let Some(e) = players.get_mut(&id) else {
            return;
        };
        let before = e.inv.clone();
        // An open container's slots.
        let cont_at = e
            .window
            .as_ref()
            .filter(|w| w.kind == crate::crafting::Kind::Container)
            .and_then(|w| Some((w.id, w.block.as_ref()?.1)));
        let window = e
            .window
            .as_ref()
            .filter(|w| w.kind == crate::crafting::Kind::Structural)
            .map(|w| {
                let mut slots = vec![w.input.clone()];
                slots.extend(
                    (0..crate::crafting::OPTION_SLOTS)
                        .map(|i| w.options.get(i).map(|(s, _)| s.clone())),
                );
                (w.id, slots, w.bench.clone())
            });
        // A processing bench's window: its fuel, input and output slots.
        let proc_at = e
            .window
            .as_ref()
            .filter(|w| w.kind == crate::crafting::Kind::Processing)
            .and_then(|w| Some((w.id, w.block.as_ref()?.1, w.bench.clone())));
        let recipes = proc_at
            .as_ref()
            .map(|(_, _, b)| self.book.processing_recipes(b, 1))
            .unwrap_or_default();
        let bench = window.as_ref().map(|(_, _, b)| b.clone());
        let proc_bench = proc_at.as_ref().map(|(_, _, b)| b.clone());
        let items = ItemsView {
            book: &self.book,
            rules: &self.rules,
            bench: bench.as_ref(),
            processing: proc_bench
                .as_ref()
                .and_then(|b| Some((b.processing.as_ref()?, recipes.as_slice()))),
            container: cont_at.is_some(),
        };
        let mut wid = window.as_ref().map(|(i, _, _)| *i);
        if let Some((cid, at)) = &cont_at
            && let Some(slots) = containers.get(at)
        {
            e.inv.attach(*cid, slots.clone());
            wid = Some(*cid);
        }
        if let Some((wid, slots, _)) = window {
            e.inv.attach(wid, slots);
        }
        if let Some((pid, at, _)) = &proc_at
            && let Some(st) = processing.get(at)
        {
            e.inv.attach(*pid, st.slots());
            wid = Some(*pid);
        }
        let prefs = e.prefs;
        e.inv.record();
        f(&mut e.inv, &items, &prefs, wid);
        let touched = e.inv.recorded();
        let mut container_changed = None;
        if let Some((cid, at)) = &cont_at
            && let Some(slots) = e.inv.detach(*cid)
        {
            if containers.get(at) != Some(&slots) {
                containers.insert(*at, slots.clone());
                container_changed = Some((*at, slots));
            }
        } else if let Some((pid, at, b)) = &proc_at
            && let Some(slots) = e.inv.detach(*pid)
            && let Some(st) = processing.get_mut(at)
            && let Some(cfg) = b.processing.as_ref()
        {
            if st.slots() != slots {
                st.set_slots(slots);
                let pi = ProcItems {
                    book: &self.book,
                    keys: &self.keys_by_id,
                };
                crate::processing::Bench {
                    cfg,
                    recipes: &recipes,
                    tier: 1,
                    items: &pi,
                }
                .update_recipe(st);
            }
            if let Some(w) = e.window.as_mut() {
                w.slots = st.slots();
                w.dirty = true;
            }
        } else if let Some(wid) = wid {
            let slots = e.inv.detach(wid).unwrap_or_default();
            let input = slots.into_iter().next().flatten();
            if let Some(w) = e.window.as_mut()
                && w.input != input
            {
                w.input = input;
                self.update_options(w);
            }
        }
        let changed = e.inv.changed_since(&before);
        e.inv_dirty.extend(changed);
        let window_touched = wid.is_some_and(|w| touched.contains(&w));
        e.inv_dirty.extend(touched.into_iter().filter(|s| *s < 0));
        if window_touched && let Some(w) = e.window.as_mut() {
            w.dirty = true;
        }
        // Everyone with the container open sees the change.
        if let Some((at, slots)) = container_changed {
            for e in players.values_mut() {
                if let Some(w) = e.window.as_mut().filter(|w| {
                    w.kind == crate::crafting::Kind::Container
                        && w.block.as_ref().is_some_and(|b| b.1 == at)
                }) {
                    w.slots = slots.clone();
                    w.dirty = true;
                }
            }
        }
    }

    /// A move that changes the kind of item in the active hotbar slot does
    /// not happen at once: the server starts a swap chain for the held
    /// item, and the move is made when that chain changes the slot (spec
    /// 18). False when the move is a plain one.
    fn swap_move(&self, id: i32, m: &proto::MoveItemStack) -> bool {
        use aerie_protocol::proto::InteractionType as IT;
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.players.get_mut(&id) else {
            return false;
        };
        let active = e.hotbar_slot;
        let to_active = m.to_section_id == inventory::HOTBAR && m.to_slot_id == active;
        let from_active = m.from_section_id == inventory::HOTBAR && m.from_slot_id == active;
        if !(to_active || from_active) || m.from_section_id >= 0 || m.to_section_id >= 0 {
            return false;
        }
        let from = e.inv.get(m.from_section_id, m.from_slot_id);
        let to = e.inv.get(m.to_section_id, m.to_slot_id);
        let (Some(from), Some(to)) = (from, to) else {
            return false;
        };
        if from.item == to.item {
            return false;
        }
        let held = e
            .inv
            .get(inventory::HOTBAR, active)
            .map(|s| s.item)
            .filter(|i| !i.is_empty() && i != "Empty");
        let Some(root) = self.chains.held_root(held.as_deref(), IT::SWAP_FROM) else {
            return false;
        };
        let slot = if to_active { m.to_slot_id } else { active };
        let ctx = crate::chains::Ctx {
            section: inventory::HOTBAR,
            slot: active,
            original: held.clone(),
            held,
            target_slot: Some(slot),
            moved: Some(crate::chains::Move {
                from: (m.from_section_id, m.from_slot_id),
                quantity: m.quantity,
                to: (m.to_section_id, m.to_slot_id),
            }),
            ..Default::default()
        };
        let data = proto::InteractionChainData {
            entity_id: -1,
            target_slot: -slot - 1,
            ..Default::default()
        };
        e.chains
            .start_server(&self.chains, IT::SWAP_FROM, ctx, root, data);
        true
    }

    /// `SetActiveSlot` (spec 14): the utility or tools slot in use. The
    /// hotbar only changes through interactions, so asking for it ends the
    /// connection, as does a slot out of range or the one already active.
    pub fn set_active_slot(&self, id: i32, payload: &[u8]) -> Result<bool> {
        let (m, _) = proto::SetActiveSlot::decode(payload, 0)?;
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.players.get_mut(&id) else {
            return Ok(true);
        };
        let (slot, cap) = match m.inventory_section_id {
            inventory::UTILITY => (&mut e.utility_slot, 4),
            -8 => (&mut e.tools_slot, 23),
            _ => return Ok(false),
        };
        if m.active_slot < -1 || m.active_slot >= cap || m.active_slot == *slot {
            return Ok(false);
        }
        *slot = m.active_slot;
        Ok(true)
    }

    /// `SyncPlayerPreferences`: where picked-up items go first.
    pub fn player_preferences(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::SyncPlayerPreferences::decode(payload, 0)?;
        use inventory::Location as L;
        if let Some(e) = self.inner.lock().unwrap().players.get_mut(&id) {
            e.prefs = inventory::Preferences {
                armor: L::from_wire(m.armor_items_preferred_pickup_location.0),
                weapon_and_tool: L::from_wire(m.weapon_and_tool_items_preferred_pickup_location.0),
                usable: L::from_wire(m.usable_items_items_preferred_pickup_location.0),
                solid_block: L::from_wire(m.solid_block_items_preferred_pickup_location.0),
                misc: L::from_wire(m.misc_items_preferred_pickup_location.0),
            };
            e.creative_distance = m.creative_interaction_distance as i32;
            e.no_physics = m.no_physics;
        }
        Ok(())
    }

    /// A stack thrown by a player (spec 11): from the eyes plus one block
    /// along the look, at `speed` along it, picked up again after 1.5 s.
    pub fn throw_item(&self, id: i32, st: Stack, speed: f32) {
        if st.n <= 0 {
            return;
        }
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.players.get(&id) else { return };
        let Some(p) = e.position.clone() else { return };
        let (pitch, yaw) = e.look.as_ref().map_or((0.0, 0.0), |d| (d.pitch, d.yaw));
        let dir = crate::trig::direction(pitch, yaw);
        let at = [p.x + dir[0], p.y + self.eye_height + dir[1], p.z + dir[2]];
        let vel = [
            ((dir[0] as f32) * speed) as f64,
            ((dir[1] as f32) * speed) as f64,
            ((dir[2] as f32) * speed) as f64,
        ];
        let born = g.tick + g.in_tick as u64;
        g.next_item = g.next_item.max(FIRST_ITEM_ID) + 1;
        let nid = g.next_item;
        let mut it = crate::items::Item::new(&st, at, vel, self.item_life(&st.item), born);
        it.pickup_delay = 1.5;
        g.items.insert(nid, it);
    }

    /// `ClientOpenWindow`: the pocket crafting window (Tab), answered with
    /// `UpdateWindow` id 0 as the official server does.
    pub fn client_open_window(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::ClientOpenWindow::decode(payload, 0)?;
        if m.r#type != proto::WindowType::POCKET_CRAFTING {
            debug!("window type {} not opened", m.r#type.0);
            return Ok(());
        }
        let w = crate::crafting::Window::fieldcraft(&self.book);
        let u = proto::UpdateWindow {
            id: 0,
            window_data: Some(w.data.text()),
            inventory: None,
            extra_resources: None,
        };
        if let Some(e) = self.inner.lock().unwrap().players.get_mut(&id) {
            e.window = Some(w);
        }
        self.send(id, packet(self::id::UPDATE_WINDOW, &u)?);
        Ok(())
    }

    /// A player used the bench block at (x, y, z) (or a part of it next to
    /// it): opens its window (spec 09). `false` when there is no bench there.
    pub fn open_bench(&self, id: i32, x: i32, y: i32, z: i32) -> Result<bool> {
        // A part of the bench: its main cell.
        let f = unpack_filler(
            self.inner
                .lock()
                .unwrap()
                .fillers
                .get(&(x, y, z))
                .copied()
                .unwrap_or(0),
        );
        let (x, y, z) = (x - f[0], y - f[1], z - f[2]);
        if self.rules.container(self.block_in_world(x, y, z)).is_some() {
            self.open_container(id, (x, y, z));
            return Ok(true);
        }
        let found = {
            let b = self.block_in_world(x, y, z);
            self.keys_by_id
                .get(&b)
                .and_then(|key| Some((key.clone(), self.book.bench(key)?.clone(), (x, y, z))))
        };
        let Some((key, bench, at)) = found else {
            return Ok(false);
        };
        let window_type = match proto::BenchType(bench.kind) {
            proto::BenchType::STRUCTURAL_CRAFTING => proto::WindowType::STRUCTURAL_CRAFTING,
            proto::BenchType::CRAFTING => proto::WindowType::BASIC_CRAFTING,
            proto::BenchType::PROCESSING if bench.processing.is_some() => {
                return self.open_processing(id, &key, bench, at);
            }
            _ => {
                // Diagram benches: not done yet (PARITY).
                info!("{key} ({}) cannot be opened yet", bench.id);
                return Ok(true);
            }
        };
        let w = {
            let mut g = self.inner.lock().unwrap();
            let Some(e) = g.players.get_mut(&id) else {
                return Ok(true);
            };
            if e.window.is_some() {
                return Ok(true);
            }
            e.next_window = e.next_window.max(0) + 1;
            let Some(mut w) = self.book.bench_window(e.next_window, &key, at, &bench) else {
                return Ok(true);
            };
            if w.kind == crate::crafting::Kind::Structural {
                let hints = self.book.inventory_hints(&bench, &e.inv);
                w.data.set("inventoryHints", json_ints(&hints));
            }
            e.window = Some(w.clone());
            w
        };
        self.sound_2d(id, bench.open_sound)?;
        let page = proto::SetPage {
            page: proto::Page::BENCH,
            can_close_through_interaction: true,
        };
        self.send(id, packet(self::id::SET_PAGE, &page)?);
        let o = proto::OpenWindow {
            id: w.id,
            window_type,
            window_data: Some(w.data.text()),
            inventory: w.section(),
            extra_resources: Some(proto::ExtraResources {
                resources: Some(Vec::new()),
            }),
        };
        self.send(id, packet(self::id::OPEN_WINDOW, &o)?);
        info!("opened {} ({}) for {id}", key, bench.id);
        Ok(true)
    }

    /// Connected blocks after a block was placed or removed at `at`
    /// (spec 23): a placed block takes the shape its rule set gives it,
    /// then the neighbours take theirs, three steps deep.
    fn connect(&self, g: &mut Inner, ground: &Ground, at: Pos, placed: bool, out: &mut Outgoing) {
        let how = How {
            quiet: true,
            ..Default::default()
        };
        let mode = {
            let w = ConnectView {
                g,
                ground,
                rules: &self.rules,
            };
            use crate::connected::Cells;
            w.at(at)
                .map(|(block, _, _)| crate::connected::update_mode(&w, block))
                .unwrap_or(proto::ConnectedBlockUpdateMode::PLACE_UPDATE)
        };
        if placed {
            // "Update, place, update": the neighbours first, without
            // touching the placed block (spec 23).
            if mode == proto::ConnectedBlockUpdateMode::UPDATE_PLACE_UPDATE {
                self.connect_around(g, ground, at, true, out);
            }
            let want = {
                let w = ConnectView {
                    g,
                    ground,
                    rules: &self.rules,
                };
                use crate::connected::Cells;
                w.at(at).filter(|c| c.2 == 0).and_then(|(block, rot, _)| {
                    Some((
                        crate::connected::desired(&w, at, block, rot, [0, 0, 0], true)?,
                        (block, rot),
                    ))
                })
            };
            if let Some((want, have)) = want
                && (want.block, want.rot) != have
            {
                self.change(g, ground, at, want.block, want.rot, how, out);
            }
            if mode == proto::ConnectedBlockUpdateMode::IGNORE_UPDATES {
                return;
            }
        }
        self.connect_around(g, ground, at, false, out);
    }

    /// The neighbour updates around `at`, three steps deep (spec 23);
    /// with `skip_start`, a change wanted at `at` itself is not made.
    fn connect_around(
        &self,
        g: &mut Inner,
        ground: &Ground,
        at: Pos,
        skip_start: bool,
        out: &mut Outgoing,
    ) {
        let how = How {
            quiet: true,
            ..Default::default()
        };
        let mut queue = std::collections::VecDeque::from([(at, 0)]);
        let mut seen = std::collections::HashSet::new();
        while let Some((here, depth)) = queue.pop_front() {
            let changes = crate::connected::neighbours(
                &ConnectView {
                    g,
                    ground,
                    rules: &self.rules,
                },
                here,
            );
            for (p, want) in changes {
                if !seen.insert(p) || p == here {
                    continue;
                }
                if !(skip_start && p == at) {
                    self.change(g, ground, p, want.block, want.rot, how, out);
                }
                for (off, block, rot) in want.extra {
                    let q = (p.0 + off[0], p.1 + off[1], p.2 + off[2]);
                    self.change(g, ground, q, block, rot, how, out);
                }
                if depth + 1 < 3 {
                    queue.push_back((p, depth + 1));
                }
            }
        }
    }

    /// A player picks a block up by using it (spec 12): a harvestable
    /// block goes, its harvest drops go straight to the player (what does
    /// not fit is thrown), and its harvest sound plays. False when the
    /// block is not harvestable.
    fn harvest(
        &self,
        g: &mut Inner,
        ground: &Ground,
        out: &mut Outgoing,
        pid: i32,
        at: Pos,
    ) -> bool {
        use crate::support::World;
        let (block, rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(at.0, at.1, at.2);
        let Some(b) = self.rules.block(block) else {
            return false;
        };
        let Some(drop) = b.harvest_drop.clone() else {
            return false;
        };
        let (own_item, sound) = (b.item.clone(), b.harvest_sound);
        let center = self.block_center(block, rot, at);
        let how = How {
            physics: true,
            ..Default::default()
        };
        if self.change(g, ground, at, 0, 0, how, out).is_some() {
            self.mark_area(g, at.0, at.1, at.2, block, rot);
        }
        self.connect(g, ground, at, false, out);
        let stacks = if drop.item.is_none() && drop.list.is_none() {
            own_item.map(|i| vec![(i, 1)]).unwrap_or_default()
        } else {
            self.rules.roll(&crate::support::Drop {
                quantity: 1,
                ..drop
            })
        };
        for (item, n) in stacks {
            let st = self.book.stack(&item, n);
            let Some(e) = g.players.get_mut(&pid) else {
                continue;
            };
            let before = e.inv.clone();
            let view = ItemsView {
                book: &self.book,
                rules: &self.rules,
                bench: None,
                processing: None,
                container: false,
            };
            let order = e.prefs.order_for(inventory::Items::kind(&view, &item));
            let left = e.inv.add_to(&view, &order, &st);
            let changed = e.inv.changed_since(&before);
            e.inv_dirty.extend(changed);
            let taken = n - left;
            if taken > 0 {
                let got = st.with_n(taken);
                self.notify_pickup(pid, &got, [f64::NAN; 3], out);
                g.next_item = g.next_item.max(FIRST_ITEM_ID) + 1;
                g.flights.insert(
                    g.next_item,
                    crate::items::Flight {
                        stack: got,
                        start: center,
                        pos: center,
                        to_player: pid,
                        life: crate::items::FLIGHT_TIME,
                        announced: false,
                        finished: false,
                    },
                );
            }
            if left > 0 {
                out.throws.push((pid, st.with_n(left)));
            }
        }
        if sound > 0 {
            self.sound_near(g, sound, center, out);
        }
        true
    }

    /// Whether a door at `at` could stand as its interaction state
    /// `state`: every cell of that state's shape is free, soft, or the
    /// door's own.
    fn door_fits(&self, g: &Inner, ground: &Ground, at: Pos, state: &str) -> bool {
        use crate::support::World;
        let view = View {
            g,
            ground,
            hub_rules: &self.rules,
        };
        let (block, rot) = view.block(at.0, at.1, at.2);
        let Some(&(_, variant)) = self
            .rules
            .block(block)
            .and_then(|b| b.states.iter().find(|(n, _)| n == state))
        else {
            return false;
        };
        self.rules.cells(variant, rot).iter().all(|c| {
            let p = (at.0 + c[0], at.1 + c[1], at.2 + c[2]);
            if !(0..crate::support::WORLD_HEIGHT).contains(&p.1) {
                return false;
            }
            let f = unpack_filler(g.fillers.get(&p).copied().unwrap_or(0));
            if (p.0 - f[0], p.1 - f[1], p.2 - f[2]) == at {
                return true;
            }
            let other = view.block(p.0, p.1, p.2).0;
            other == 0 || self.rules.block(other).is_some_and(|o| !o.solid)
        })
    }

    /// Switches a door to an interaction state: soft blocks where its new
    /// shape goes break with their drops. The block it becomes.
    fn switch_door(
        &self,
        g: &mut Inner,
        ground: &Ground,
        at: Pos,
        state: &str,
        out: &mut Outgoing,
    ) -> Option<i32> {
        use crate::support::World;
        let (block, rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(at.0, at.1, at.2);
        let &(_, variant) = self
            .rules
            .block(block)?
            .states
            .iter()
            .find(|(n, _)| n == state)?;
        for c in self.rules.cells(variant, rot) {
            if c == [0, 0, 0] {
                continue;
            }
            let p = (at.0 + c[0], at.1 + c[1], at.2 + c[2]);
            let f = unpack_filler(g.fillers.get(&p).copied().unwrap_or(0));
            if (p.0 - f[0], p.1 - f[1], p.2 - f[2]) == at {
                continue;
            }
            let other = View {
                g,
                ground,
                hub_rules: &self.rules,
            }
            .block(p.0, p.1, p.2)
            .0;
            let Some(o) = self.rules.block(other).filter(|o| other != 0 && !o.solid) else {
                continue;
            };
            let drop = o.physics_drop.clone().or_else(|| o.soft_drop.clone());
            let drops = drop.map_or((None, None, 1), |d| (d.item, d.list, 1));
            self.break_with(g, ground, p, Some(drops), true, false, out);
        }
        self.set_block_state(g, ground, at, state, out);
        Some(variant)
    }

    /// A door's state from its block's interaction state: 0 closed, 1
    /// opened in, 2 opened out.
    fn door_state(&self, block: i32) -> i8 {
        match self.rules.state_of(block) {
            "OpenDoorOut" => 1,
            "OpenDoorIn" => 2,
            _ => 0,
        }
    }

    /// The interaction state a door switches to, going from one state to
    /// another.
    fn door_switch(from: i8, to: i8) -> &'static str {
        match (to, from) {
            (0, 1) => "CloseDoorOut",
            (0, 2) => "CloseDoorIn",
            (1, _) => "OpenDoorOut",
            _ => "OpenDoorIn",
        }
    }

    /// The door that makes a double door with the one at `at`: next to
    /// it on its hinge-far side, the same kind of door turned the other
    /// way, in the state asked for. Its position, block and state.
    fn double_door(
        &self,
        g: &Inner,
        ground: &Ground,
        at: Pos,
        block: i32,
        rot: i32,
        state: i8,
    ) -> Option<(Pos, i32, i8)> {
        use crate::support::World;
        let base = self.rules.base(block)?;
        let shape = *self.blocks.get(&base.key.to_lowercase())?;
        let width = self.rules.cells(shape, 0).iter().map(|c| c[0]).max()? + 1;
        let off = width * 2 - 1;
        let yaw = rot % 4;
        // The offset turned by the door's yaw (counter-clockwise).
        let (dx, dz) = match yaw {
            0 => (off, 0),
            1 => (0, -off),
            2 => (-off, 0),
            _ => (0, off),
        };
        let p = (at.0 + dx, at.1, at.2 + dz);
        let (other, other_rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(p.0, p.1, p.2);
        let other_base = self.rules.base(other)?;
        other_base.door?;
        if other_rot % 4 != (yaw + 2) % 4
            || self.door_state(other) != state
            || g.fillers.get(&p).copied().unwrap_or(0) != 0
            || other_base.hitbox != base.hitbox
        {
            return None;
        }
        Some((p, other, self.door_state(other)))
    }

    /// A player uses a door (spec 22). False when it cannot move.
    fn use_door(
        &self,
        g: &mut Inner,
        ground: &Ground,
        out: &mut Outgoing,
        pid: i32,
        at: Pos,
        horizontal: bool,
    ) -> bool {
        use crate::support::World;
        let (block, rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(at.0, at.1, at.2);
        let state = self.door_state(block);
        let opposite = |s: i8| match s {
            1 => 2,
            2 => 1,
            s => s,
        };
        let yaw = (rot % 4) as f64 * std::f64::consts::FRAC_PI_2;
        let wanted = if state != 0 {
            0
        } else {
            // Opens away from the player: out when the player stands in
            // front of it.
            let in_front = g
                .players
                .get(&pid)
                .and_then(|e| e.position.as_ref())
                .is_some_and(|p| {
                    let d = [p.x - (at.0 as f64 + 0.5), p.z - (at.2 as f64 + 0.5)];
                    d[0] * yaw.sin() + d[1] * yaw.cos() < 0.0
                });
            if !horizontal && in_front { 2 } else { 1 }
        };
        // The way asked for, else the other way, with the second door of
        // a double door able to follow; else the door shows itself
        // blocked.
        let partner = self
            .double_door(g, ground, at, block, rot, state)
            .map(|d| d.0);
        let way = Self::door_switch(state, wanted);
        let other_way = Self::door_switch(state, opposite(wanted));
        let fits = |p: Pos, s: &str| self.door_fits(g, ground, p, s);
        let chosen = if fits(at, way) {
            if horizontal || partner.is_none_or(|p| fits(p, other_way)) {
                Some(wanted)
            } else if fits(at, other_way) && partner.is_some_and(|p| fits(p, way)) {
                Some(opposite(wanted))
            } else {
                self.set_block_state(g, ground, at, "DoorBlocked", out);
                None
            }
        } else if fits(at, other_way) && !horizontal {
            if partner.is_some_and(|p| !fits(p, way)) {
                self.set_block_state(g, ground, at, "DoorBlocked", out);
                None
            } else {
                Some(opposite(wanted))
            }
        } else {
            if wanted != 0 {
                self.set_block_state(g, ground, at, "DoorBlocked", out);
            }
            None
        };
        let Some(wanted) = chosen else { return false };
        let switched = Self::door_switch(state, wanted);
        let moved = self.switch_door(g, ground, at, switched, out).is_some();
        // The other door of a double door follows, the opposite way (a
        // horizontal one the same way).
        let mut double = false;
        if let Some((p, other, other_state)) =
            self.double_door(g, ground, at, block, rot, opposite(state))
        {
            let other_horizontal = self.rules.base(other).and_then(|b| b.door) == Some(true);
            let to = if other_horizontal {
                wanted
            } else {
                opposite(wanted)
            };
            self.switch_door(g, ground, p, Self::door_switch(other_state, to), out);
            double = true;
        }
        if moved {
            let base = self.rules.base(block);
            let sound = base
                .and_then(|b| b.state_sounds.iter().find(|(n, _)| n == switched))
                .map_or(0, |s| s.1);
            if sound > 0 {
                // At the middle of the door's own (closed) shape; of both
                // doors for a double door.
                let shape = base
                    .and_then(|b| self.blocks.get(&b.key.to_lowercase()))
                    .copied()
                    .unwrap_or(block);
                let mut c = self.block_center(shape, rot, at);
                if double {
                    let half = c[0] - at.0 as f64;
                    let turn = (rot % 4) as f64 * std::f64::consts::FRAC_PI_2;
                    c[0] += half * turn.cos();
                    c[2] -= half * turn.sin();
                }
                self.sound_near(g, sound, c, out);
            }
        }
        true
    }

    /// Opens the window of a block that holds items (spec 21): the page,
    /// the window with the slots, the open sound, and the block shown open
    /// when this is its first window.
    fn open_container(&self, id: i32, at: Pos) {
        self.world_step(|hub, g, ground, out| {
            use crate::support::World;
            let (block, rot) = View {
                g,
                ground,
                hub_rules: &hub.rules,
            }
            .block(at.0, at.1, at.2);
            let Some((c, base)) = hub.rules.container(block) else {
                return;
            };
            if g.players.get(&id).is_none_or(|e| e.window.is_some()) {
                return;
            }
            let slots = g.containers.entry(at).or_default();
            slots.resize(c.capacity, None);
            let slots = slots.clone();
            let first = !g.players.values().any(|e| hub.has_container_open(e, at));
            let Some(e) = g.players.get_mut(&id) else {
                return;
            };
            e.next_window = e.next_window.max(0) + 1;
            let item = base.item.clone().unwrap_or_else(|| base.key.clone());
            let w = crate::crafting::Window::container(e.next_window, &base.key, at, &item, slots);
            let page = proto::SetPage {
                page: proto::Page::BENCH,
                can_close_through_interaction: true,
            };
            let open = proto::OpenWindow {
                id: w.id,
                window_type: proto::WindowType::CONTAINER,
                window_data: Some(w.data.text()),
                inventory: w.section(),
                extra_resources: None,
            };
            e.window = Some(w);
            for p in [
                packet(self::id::SET_PAGE, &page),
                packet(self::id::OPEN_WINDOW, &open),
            ]
            .into_iter()
            .flatten()
            {
                out.to.push((id, p));
            }
            if c.open_sound > 0 {
                hub.sound_near(g, c.open_sound, hub.block_center(block, rot, at), out);
            }
            if first {
                hub.set_block_state(g, ground, at, "OpenWindow", out);
            }
        });
    }

    fn has_container_open(&self, e: &Entry, at: Pos) -> bool {
        e.window.as_ref().is_some_and(|w| {
            w.kind == crate::crafting::Kind::Container
                && w.block.as_ref().is_some_and(|b| b.1 == at)
        })
    }

    /// A container's window closed: with no window left on it the block
    /// is shown closed (unless it stays open while empty), and the close
    /// sound plays.
    fn container_closed(&self, g: &mut Inner, ground: &Ground, at: Pos, out: &mut Outgoing) {
        use crate::support::World;
        let (block, rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(at.0, at.1, at.2);
        let Some((c, _)) = self.rules.container(block) else {
            return;
        };
        let (close_sound, stay) = (c.close_sound, c.stay_open_when_empty);
        if !g.players.values().any(|e| self.has_container_open(e, at)) {
            let empty = g
                .containers
                .get(&at)
                .is_none_or(|s| s.iter().all(Option::is_none));
            if stay && empty {
                return;
            }
            self.set_block_state(g, ground, at, "CloseWindow", out);
        }
        if close_sound > 0 {
            self.sound_near(g, close_sound, self.block_center(block, rot, at), out);
        }
    }

    /// A container block is gone: what it held drops, its windows close.
    fn drop_container(&self, g: &mut Inner, at: Pos, out: &mut Outgoing) {
        let Some(slots) = g.containers.remove(&at) else {
            return;
        };
        let stacks: Vec<(String, i32)> =
            slots.into_iter().flatten().map(|s| (s.item, s.n)).collect();
        if !stacks.is_empty() {
            out.drops
                .push((stacks, [at.0 as f64 + 0.5, at.1 as f64, at.2 as f64 + 0.5]));
        }
        for (pid, e) in g.players.iter_mut() {
            if self.has_container_open(e, at)
                && let Some(w) = e.window.take()
                && let Ok(p) = packet(self::id::CLOSE_WINDOW, &proto::CloseWindow { id: w.id })
            {
                out.to.push((*pid, p));
            }
        }
    }

    /// Opens a processing bench's window (spec 16): the bench's state is
    /// made on first use; the window starts with the fuel shown.
    fn open_processing(
        &self,
        id: i32,
        key: &str,
        bench: crate::crafting::Bench,
        at: Pos,
    ) -> Result<bool> {
        let Some(cfg) = bench.processing.clone() else {
            return Ok(true);
        };
        let w = {
            let mut g = self.inner.lock().unwrap();
            let Inner {
                players,
                processing,
                ..
            } = &mut *g;
            let Some(e) = players.get_mut(&id) else {
                return Ok(true);
            };
            if e.window.is_some() {
                return Ok(true);
            }
            let st = processing
                .entry(at)
                .or_insert_with(|| crate::processing::State::new(&cfg, 1));
            let recipes = self.book.processing_recipes(&bench, 1);
            let pi = ProcItems {
                book: &self.book,
                keys: &self.keys_by_id,
            };
            let progress = crate::processing::Bench {
                cfg: &cfg,
                recipes: &recipes,
                tier: 1,
                items: &pi,
            }
            .progress(st);
            e.next_window = e.next_window.max(0) + 1;
            let Some(mut w) =
                self.book
                    .processing_window(e.next_window, key, at, &bench, st, progress)
            else {
                return Ok(true);
            };
            let (fuel, max) = st.fuel_shown();
            w.data.set("fuelTime", crate::crafting::fmt_f32(fuel));
            w.data.set("maxFuel", max.to_string());
            w.data.set(
                "processingFuelSlots",
                crate::processing::mask(&st.processing_fuel_slots).to_string(),
            );
            e.window = Some(w.clone());
            w
        };
        let page = proto::SetPage {
            page: proto::Page::BENCH,
            can_close_through_interaction: true,
        };
        self.send(id, packet(self::id::SET_PAGE, &page)?);
        let o = proto::OpenWindow {
            id: w.id,
            window_type: proto::WindowType::PROCESSING,
            window_data: Some(w.data.text()),
            inventory: w.section(),
            extra_resources: Some(proto::ExtraResources {
                resources: Some(Vec::new()),
            }),
        };
        self.send(id, packet(self::id::OPEN_WINDOW, &o)?);
        self.sound_2d(id, bench.open_sound)?;
        info!("opened {} ({}) for {id}", key, bench.id);
        Ok(true)
    }

    /// A processing bench is gone: everything inside falls out at the
    /// block's bottom centre, with the whole seconds of fuel left as the
    /// bench's fuel drop item.
    fn drop_processing(&self, g: &mut Inner, at: Pos, out: &mut Outgoing) {
        let Some(mut st) = g.processing.remove(&at) else {
            return;
        };
        let mut stacks: Vec<(String, i32)> = st.all().into_iter().map(|s| (s.item, s.n)).collect();
        for (item, mut n) in std::mem::take(&mut st.dropped_fuel) {
            let max = self.book.max_stack(&item).max(1);
            while n > 0 {
                let k = n.min(max);
                stacks.push((item.clone(), k));
                n -= k;
            }
        }
        if !stacks.is_empty() {
            out.drops
                .push((stacks, [at.0 as f64 + 0.5, at.1 as f64, at.2 as f64 + 0.5]));
        }
        // Windows open on it close.
        for (pid, e) in g.players.iter_mut() {
            if e.window.as_ref().is_some_and(|w| {
                w.kind == crate::crafting::Kind::Processing
                    && w.block.as_ref().is_some_and(|(_, p)| *p == at)
            }) && let Some(w) = e.window.take()
                && let Ok(p) = packet(self::id::CLOSE_WINDOW, &proto::CloseWindow { id: w.id })
            {
                out.to.push((*pid, p));
            }
        }
    }

    /// The processing benches for one tick (spec 16): each burns fuel and
    /// crafts as the official bench block, switches its block's state,
    /// plays its sounds, throws out what does not fit and updates the
    /// windows open on it. A bench whose block is gone drops what it held.
    fn processing_step(&self, g: &mut Inner, ground: &Ground, dt: f32, out: &mut Outgoing) {
        use crate::support::World;
        // Benches from the world save come back with what they held.
        for (at, text) in std::mem::take(&mut g.saved_states) {
            let block = View {
                g,
                ground,
                hub_rules: &self.rules,
            }
            .block(at.0, at.1, at.2)
            .0;
            if let Some(cfg) = self
                .keys_by_id
                .get(&block)
                .and_then(|k| self.book.bench(k))
                .and_then(|b| b.processing.as_ref())
            {
                let mut st = crate::processing::State::new(cfg, 1);
                st.load(&text);
                g.processing.insert(at, st);
            }
        }
        let places: Vec<Pos> = g.processing.keys().copied().collect();
        for at in places {
            let (block, rot) = View {
                g,
                ground,
                hub_rules: &self.rules,
            }
            .block(at.0, at.1, at.2);
            let bench = self
                .keys_by_id
                .get(&block)
                .and_then(|k| self.book.bench(k))
                .filter(|b| b.processing.is_some())
                .cloned();
            let Some(bench) = bench else {
                self.drop_processing(g, at, out);
                continue;
            };
            let cfg = bench.processing.as_ref().unwrap();
            let current = self
                .rules
                .block(block)
                .and_then(|b| b.states.iter().find(|(_, i)| *i == block))
                .map_or("default".to_owned(), |(n, _)| n.clone());
            let recipes = self.book.processing_recipes(&bench, 1);
            let pi = ProcItems {
                book: &self.book,
                keys: &self.keys_by_id,
            };
            let pb = crate::processing::Bench {
                cfg,
                recipes: &recipes,
                tier: 1,
                items: &pi,
            };
            let sounds = crate::processing::Sounds {
                failed: bench.failed_sound,
                end: cfg.end_sound,
                completed: bench.completed_sound,
            };
            let Some(st) = g.processing.get_mut(&at) else {
                continue;
            };
            st.fuel_drop = cfg.fuel_drop.clone();
            let ev = pb.tick(st, &current, dt, sounds);
            let st = st.clone();
            if let Some(state) = ev.state {
                self.set_block_state(g, ground, at, state, out);
            }
            let center = self.block_center(block, rot, at);
            for s in ev.sounds {
                if s > 0 {
                    self.sound_near(g, s, center, out);
                }
            }
            if !ev.eject.is_empty() {
                let (dir, depth) = self.rules.front(block, rot);
                let pos = match depth {
                    Some(d) => {
                        let off = d / 2.0 + 0.1f32 as f64;
                        [
                            center[0] + dir[0] * off,
                            center[1],
                            center[2] + dir[1] * off,
                        ]
                    }
                    None => [at.0 as f64 + 0.5, at.1 as f64, at.2 as f64 + 0.5],
                };
                let born = g.tick + g.in_tick as u64;
                for stack in ev.eject {
                    let vx = (dir[0] * 2.0 + 2.0 * (rand::random::<f64>() - 0.5)) as f32;
                    let vz = (dir[1] * 2.0 + 2.0 * (rand::random::<f64>() - 0.5)) as f32;
                    g.next_item = g.next_item.max(FIRST_ITEM_ID) + 1;
                    let nid = g.next_item;
                    let life = self.item_life(&stack.item);
                    g.items.insert(
                        nid,
                        crate::items::Item::new(
                            &stack,
                            pos,
                            [vx as f64, 3.25, vz as f64],
                            life,
                            born,
                        ),
                    );
                }
            }
            // The windows open on this bench.
            let progress = st.progress_sent;
            for e in g.players.values_mut() {
                let Some(w) = e.window.as_mut().filter(|w| {
                    w.kind == crate::crafting::Kind::Processing
                        && w.block.as_ref().is_some_and(|(_, p)| *p == at)
                }) else {
                    continue;
                };
                let before = w.data.text();
                w.data.set("active", st.active.to_string());
                if let Some(p) = progress {
                    w.data.set("progress", crate::crafting::fmt_f32(p));
                }
                if let Some((f, max)) = ev.fuel {
                    w.data.set("fuelTime", crate::crafting::fmt_f32(f));
                    w.data.set("maxFuel", max.to_string());
                }
                w.data.set(
                    "processingSlots",
                    crate::processing::mask(&st.processing_slots).to_string(),
                );
                if ev.fuel.is_some() || !st.active {
                    w.data.set(
                        "processingFuelSlots",
                        crate::processing::mask(&st.processing_fuel_slots).to_string(),
                    );
                }
                let slots = st.slots();
                if w.data.text() != before || w.slots != slots {
                    w.slots = slots;
                    w.dirty = true;
                }
            }
            if let Some(s) = g.processing.get_mut(&at) {
                s.progress_sent = None;
            }
        }
    }

    /// A UI sound for one player (none for index 0).
    fn sound_2d(&self, id: i32, index: i32) -> Result<()> {
        if index > 0 {
            self.send(id, sound_2d_packet(index)?);
        }
        Ok(())
    }

    /// `CloseWindow` from the client: confirmed, then the bench's close
    /// steps (spec 09): its block back to its default state, the queue
    /// cancelled (inputs of the running craft given back, the failed
    /// sound), the close sound, a structural input given back.
    pub fn close_window(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::CloseWindow::decode(payload, 0)?;
        self.send(id, packet(self::id::CLOSE_WINDOW, &m)?);
        self.world_step(|hub, g, ground, out| {
            let Some(e) = g.players.get_mut(&id) else {
                return;
            };
            if !e.window.as_ref().is_some_and(|w| w.id == m.id) {
                return;
            }
            let w = e.window.take().unwrap();
            let refunded = hub.cancel_crafting(e);
            let input = w.input.clone();
            if w.kind == crate::crafting::Kind::Container {
                if let Some((_, at)) = &w.block {
                    hub.container_closed(g, ground, *at, out);
                }
                return;
            }
            if let Some((_, at)) = &w.block {
                hub.set_block_state(g, ground, *at, "default", out);
            }
            if refunded
                && w.bench.failed_sound > 0
                && let Ok(p) = sound_2d_packet(w.bench.failed_sound)
            {
                out.to.push((id, p));
            }
            if w.bench.close_sound > 0
                && let Ok(p) = sound_2d_packet(w.bench.close_sound)
            {
                out.to.push((id, p));
            }
            if let Some(st) = input
                && let Some(e) = g.players.get_mut(&id)
            {
                let before = e.inv.clone();
                let left = e.inv.add(&st, hub.book.max_stack(&st.item));
                let changed = e.inv.changed_since(&before);
                e.inv_dirty.extend(changed);
                if left > 0 {
                    out.throws.push((id, st.with_n(left)));
                }
            }
        });
        Ok(())
    }

    /// Cancels a player's crafting queue: the running craft's inputs come
    /// back. `true` if there was a queue.
    fn cancel_crafting(&self, e: &mut Entry) -> bool {
        let jobs = std::mem::take(&mut e.queue.jobs);
        let Some(first) = jobs.into_iter().next() else {
            return false;
        };
        if let Some(taken) = first.taken.get((first.started - 1).max(0) as usize)
            && first.started > first.completed
        {
            let before = e.inv.clone();
            for (item, n) in taken {
                e.inv
                    .add(&self.book.stack(item, *n), self.book.max_stack(item));
            }
            let changed = e.inv.changed_since(&before);
            e.inv_dirty.extend(changed);
        }
        true
    }

    /// Switches a bench block to one of its interaction states (no
    /// particles, no physics), as the official crafting does.
    fn set_block_state(
        &self,
        g: &mut Inner,
        ground: &Ground,
        at: Pos,
        state: &str,
        out: &mut Outgoing,
    ) {
        use crate::support::World;
        let (cur, rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(at.0, at.1, at.2);
        let Some(b) = self.rules.block(cur) else {
            return;
        };
        let Some(&(_, to)) = b.states.iter().find(|(n, _)| n == state) else {
            return;
        };
        let how = How {
            quiet: true,
            ..Default::default()
        };
        self.change(g, ground, at, to, rot, how, out);
    }

    /// `SendWindowAction` (spec 09): crafting, picking a structural option,
    /// cycling a structural input through its block group.
    pub fn window_action(&self, id: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::SendWindowAction::decode(payload, 0)?;
        let mut unknown = None;
        self.world_step(|hub, g, ground, out| {
            let Some(e) = g.players.get_mut(&id) else {
                return;
            };
            let Some(w) = e.window.as_ref().filter(|w| w.id == m.id).cloned() else {
                debug!("action in window {} that is not open", m.id);
                return;
            };
            use crate::crafting::Kind;
            match (&m.action, w.kind) {
                (proto::WindowAction::SelectSlotAction(a), Kind::Structural) => {
                    let slot = a.slot.clamp(0, crate::crafting::OPTION_SLOTS as i32 - 1);
                    if let Some(w) = e.window.as_mut()
                        && slot != w.selected
                    {
                        w.selected = slot;
                        w.data.set("selected", slot.to_string());
                        w.dirty = true;
                    }
                }
                (proto::WindowAction::ChangeBlockAction(a), Kind::Structural) => {
                    if w.bench.cycling {
                        hub.cycle_input(id, e, a.down, out);
                    }
                }
                (proto::WindowAction::CraftRecipeAction(a), Kind::Structural) => {
                    let Some((_, recipe)) = w.options.get(w.selected as usize).cloned() else {
                        return;
                    };
                    if let Some(p) = hub
                        .book
                        .recipe(&recipe)
                        .and_then(|r| r.primary_output.as_ref())
                        .and_then(|o| o.item_id.clone())
                    {
                        let s = hub.book.drop_sound(&p);
                        if s > 0
                            && let Ok(p) = sound_2d_packet(s)
                        {
                            out.to.push((id, p));
                        }
                    }
                    if hub.queue_craft(e, &w, &recipe, a.quantity, true) {
                        let left = e.queue.remaining();
                        if let Some(w) = e.window.as_mut() {
                            w.data.set("queueSize", left.to_string());
                        }
                    }
                    if let Some(w) = e.window.as_mut() {
                        w.dirty = true;
                    }
                }
                (proto::WindowAction::CraftRecipeAction(a), Kind::Pocket) => {
                    let Some(recipe) = a.recipe_id.clone() else {
                        return;
                    };
                    if hub.book.recipe(&recipe).is_none() {
                        unknown = Some(recipe);
                        return;
                    }
                    hub.craft_now(id, e, &w, &recipe, a.quantity, out);
                    let s = hub.book.sound("SFX_Player_Craft_Item_Inventory");
                    if s > 0
                        && let Ok(p) = sound_2d_packet(s)
                    {
                        out.to.push((id, p));
                    }
                }
                (proto::WindowAction::CraftRecipeAction(a), Kind::Simple) => {
                    let Some(recipe) = a.recipe_id.clone() else {
                        return;
                    };
                    if hub.book.recipe(&recipe).is_none() {
                        unknown = Some(recipe);
                        return;
                    }
                    let timed = hub.book.seconds(&recipe) > 0.0;
                    let accepted = if timed {
                        let ok = hub.queue_craft(e, &w, &recipe, a.quantity, false);
                        if ok {
                            let left = e.queue.remaining();
                            if let Some(w) = e.window.as_mut() {
                                w.data.set("queueSize", left.to_string());
                            }
                        }
                        ok
                    } else {
                        hub.craft_now(id, e, &w, &recipe, a.quantity, out)
                    };
                    if let Some(w) = e.window.as_mut() {
                        w.extra_dirty = true;
                        w.dirty = true;
                    }
                    if accepted && let Some((_, at)) = &w.block {
                        let state = if timed {
                            "CraftCompleted"
                        } else {
                            "CraftCompletedInstant"
                        };
                        let at = *at;
                        hub.set_block_state(g, ground, at, state, out);
                        if w.bench.completed_sound > 0 {
                            let center = [at.0 as f64 + 0.5, at.1 as f64 + 0.5, at.2 as f64 + 0.5];
                            if let Ok(p) = sound_3d_packet(
                                w.bench.completed_sound,
                                proto::SoundCategory::SFX,
                                center,
                            ) {
                                out.all.push(p);
                            }
                        }
                    }
                }
                (proto::WindowAction::SetActiveAction(a), Kind::Processing) => {
                    // On needs fuel; a refusal resends the window.
                    let Some((_, at)) = w.block.clone() else {
                        return;
                    };
                    let Some(cfg) = w.bench.processing.as_ref() else {
                        return;
                    };
                    let changed = g.processing.get_mut(&at).map(|st| {
                        let c = st.set_active(a.state, cfg);
                        if c {
                            let recipes = hub.book.processing_recipes(&w.bench, 1);
                            let pi = ProcItems {
                                book: &hub.book,
                                keys: &hub.keys_by_id,
                            };
                            crate::processing::Bench {
                                cfg,
                                recipes: &recipes,
                                tier: 1,
                                items: &pi,
                            }
                            .update_recipe(st);
                        }
                        (c, st.active)
                    });
                    if let Some((c, active)) = changed {
                        for e in g.players.values_mut() {
                            if let Some(w) = e.window.as_mut().filter(|x| {
                                x.kind == Kind::Processing
                                    && x.block.as_ref().is_some_and(|(_, p)| *p == at)
                            }) {
                                w.data.set("active", active.to_string());
                                if !active {
                                    w.data.set("processingSlots", "0".into());
                                    w.data.set("processingFuelSlots", "0".into());
                                }
                                w.dirty = true;
                            }
                        }
                        if !c
                            && let Some(w) = g.players.get_mut(&id).and_then(|e| e.window.as_mut())
                        {
                            w.dirty = true;
                        }
                    }
                }
                (other, _) => debug!("window action not handled: {other:?}"),
            }
        });
        if let Some(r) = unknown {
            warn!("{id} asked for unknown recipe {r}");
        }
        Ok(())
    }

    /// An instant craft (spec 09): checks, takes the inputs (not in
    /// creative), gives the outputs; tells the player what is missing.
    fn craft_now(
        &self,
        pid: i32,
        e: &mut Entry,
        w: &crate::crafting::Window,
        recipe: &str,
        requested: i32,
        out: &mut Outgoing,
    ) -> bool {
        let quantity = requested.min(self.book.ceiling(recipe));
        if quantity < 1
            || self
                .book
                .allowed(recipe, w.bench.kind, &w.bench.id)
                .is_err()
        {
            return false;
        }
        if w.kind == crate::crafting::Kind::Simple
            && e.queue.jobs.front().is_some_and(|j| j.recipe != recipe)
        {
            return false;
        }
        let before = e.inv.clone();
        if !e.creative {
            for _ in 0..quantity {
                if self.book.take_unit(&mut e.inv, recipe).is_none() {
                    e.inv = before;
                    self.missing_ingredient(pid, recipe, out);
                    return false;
                }
            }
        }
        for (item, n) in self.book.outputs(recipe, quantity) {
            let st = self.book.stack(&item, n);
            let left = e.inv.add(&st, self.book.max_stack(&item));
            if left > 0 {
                out.throws.push((pid, st.with_n(left)));
            }
        }
        let changed = e.inv.changed_since(&before);
        e.inv_dirty.extend(changed);
        true
    }

    /// Queues a timed craft (spec 09); `from_input`: a structural bench's
    /// input slot pays.
    fn queue_craft(
        &self,
        e: &mut Entry,
        w: &crate::crafting::Window,
        recipe: &str,
        requested: i32,
        from_input: bool,
    ) -> bool {
        if self
            .book
            .allowed(recipe, w.bench.kind, &w.bench.id)
            .is_err()
        {
            return false;
        }
        if w.kind == crate::crafting::Kind::Simple
            && e.queue.jobs.front().is_some_and(|j| j.recipe != recipe)
        {
            return false;
        }
        let quantity = requested.min(self.book.ceiling(recipe));
        if quantity < 1 || e.queue.jobs.len() >= 32 {
            return false;
        }
        e.queue.jobs.push_back(crate::crafting::Job {
            recipe: recipe.to_owned(),
            quantity,
            started: 0,
            completed: 0,
            seconds_done: 0.0,
            seconds: self.book.seconds(recipe),
            from_input,
            taken: Vec::new(),
        });
        true
    }

    fn missing_ingredient(&self, pid: i32, recipe: &str, out: &mut Outgoing) {
        let Some(item) = self
            .book
            .recipe(recipe)
            .and_then(|r| r.primary_output.as_ref())
            .and_then(|o| o.item_id.clone())
        else {
            return;
        };
        let msg = proto::FormattedMessage {
            message_id: Some("server.general.crafting.missingIngredient".into()),
            message_params: Some(vec![(
                "item".into(),
                proto::FormattedMessage {
                    message_id: Some(self.book.item_name(&item)),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };
        let n = proto::Notification {
            message: Some(msg),
            style: proto::NotificationStyle::DANGER,
            ..Default::default()
        };
        if let Ok(p) = packet(self::id::NOTIFICATION, &n) {
            out.to.push((pid, p));
        }
    }

    /// A structural input cycled through its block group (spec 09).
    fn cycle_input(&self, pid: i32, e: &mut Entry, down: bool, out: &mut Outgoing) {
        let Some(w) = e.window.as_mut() else { return };
        let Some(input) = w.input.clone() else {
            return;
        };
        let (item, n) = (input.item.clone(), input.n);
        let Some(group) = self.book.group_of(&item) else {
            return;
        };
        let Some(i) = group.iter().position(|x| *x == item) else {
            return;
        };
        let len = group.len();
        let next = if down {
            (i + len - 1) % len
        } else {
            (i + 1) % len
        };
        let next = group[next].clone();
        w.input = Some(self.book.stack(&next, n));
        let s = self.book.drop_sound(&next);
        self.update_options(w);
        if s > 0
            && let Ok(p) = sound_2d_packet(s)
        {
            out.to.push((pid, p));
        }
    }

    /// The options of a structural window after its input changed.
    fn update_options(&self, w: &mut crate::crafting::Window) {
        w.dirty = true;
        w.options.clear();
        let Some(Stack { item, .. }) = w.input.clone() else {
            return;
        };
        let (options, divider) = self.book.structural_options(&w.bench, &item);
        if options.is_empty() {
            return;
        }
        w.data.set("dividerIndex", divider.to_string());
        let ids: Vec<String> = options.iter().map(|(_, r)| r.clone()).collect();
        w.data.set(
            "optionSlotRecipes",
            format!(
                "[{}]",
                ids.iter()
                    .map(|r| format!("{r:?}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
        w.options = options;
    }

    /// The crafting queues for one tick (spec 09), then the windows that
    /// changed go out.
    fn crafting_step(&self, g: &mut Inner, ground: &Ground, dt: f32, out: &mut Outgoing) {
        let ids: Vec<i32> = g.players.keys().copied().collect();
        for pid in ids {
            let mut left_dt = dt;
            let mut emptied = None;
            {
                let Some(e) = g.players.get_mut(&pid) else {
                    continue;
                };
                while left_dt > 0.0 && !e.queue.jobs.is_empty() {
                    let creative = e.creative;
                    let mut job = e.queue.jobs.pop_front().unwrap();
                    if job.started < job.quantity && job.started <= job.completed {
                        job.started += 1;
                        if !creative {
                            match self.take_job_unit(e, &job) {
                                Some(t) => job.taken.push(t),
                                None => {
                                    self.missing_ingredient(pid, &job.recipe.clone(), out);
                                    let left = e.queue.remaining();
                                    if let Some(w) = e.window.as_mut() {
                                        w.data.set("queueSize", left.to_string());
                                    }
                                    if e.queue.jobs.is_empty() {
                                        emptied = e.window.as_ref().and_then(|w| w.block.clone());
                                    }
                                    continue;
                                }
                            }
                        } else {
                            job.taken.push(Vec::new());
                        }
                    }
                    job.seconds_done += left_dt;
                    let pct = if job.seconds <= 0.0 {
                        1.0
                    } else {
                        (job.seconds_done / job.seconds).min(1.0)
                    };
                    if let Some(w) = e.window.as_mut() {
                        w.progress(pct);
                    }
                    left_dt = 0.0;
                    let mut done = false;
                    if job.seconds_done >= job.seconds {
                        left_dt = job.seconds_done - job.seconds;
                        job.completed += 1;
                        job.seconds_done = 0.0;
                        let before = e.inv.clone();
                        for (item, n) in self.book.outputs(&job.recipe, 1) {
                            let st = self.book.stack(&item, n);
                            let left = e.inv.add(&st, self.book.max_stack(&item));
                            if left > 0 {
                                out.throws.push((pid, st.with_n(left)));
                            }
                        }
                        let changed = e.inv.changed_since(&before);
                        e.inv_dirty.extend(changed);
                        done = job.completed == job.quantity;
                    }
                    let finished_unit = job.seconds_done == 0.0;
                    if !done {
                        e.queue.jobs.push_front(job);
                    }
                    if finished_unit {
                        let left = e.queue.remaining();
                        if let Some(w) = e.window.as_mut() {
                            w.data.set("queueSize", left.to_string());
                        }
                        if e.queue.jobs.is_empty() {
                            emptied = e.window.as_ref().and_then(|w| w.block.clone());
                        }
                    }
                }
            }
            if let Some((_, at)) = emptied {
                self.set_block_state(g, ground, at, "default", out);
            }
        }
    }

    /// What players are sent about their own inventory and window (spec 09,
    /// 14, 16): the changed inventory sections, then the changed window.
    /// Runs at the start of a tick for what client packets changed since
    /// the last one (the official server handles packets after its systems
    /// and sends at the end of that tick), and at its end for the tick's
    /// own changes.
    fn send_player_updates(&self, g: &mut Inner, out: &mut Outgoing) {
        for (pid, e) in g.players.iter_mut() {
            let pid = *pid;
            // A structural window follows the player's hotbar and storage.
            if (e.inv_dirty.contains(&inventory::HOTBAR)
                || e.inv_dirty.contains(&inventory::STORAGE))
                && let Some(w) = e.window.as_mut()
                && w.kind == crate::crafting::Kind::Structural
            {
                let hints = self.book.inventory_hints(&w.bench, &e.inv);
                w.data.set("inventoryHints", json_ints(&hints));
                w.dirty = true;
            }
            if !e.inv_dirty.is_empty() {
                if e.inv_dirty.contains(&inventory::ARMOR) {
                    e.mods_dirty = true;
                }
                let which: Vec<i32> = std::mem::take(&mut e.inv_dirty).into_iter().collect();
                if let Ok(p) = packet(id::UPDATE_PLAYER_INVENTORY, &e.inv.packet_of(&which)) {
                    out.to.push((pid, p));
                }
            }
            if let Some(w) = e.window.as_mut()
                && w.dirty
                && w.id != 0
            {
                w.dirty = false;
                let extra = std::mem::take(&mut w.extra_dirty).then(|| proto::ExtraResources {
                    resources: Some(Vec::new()),
                });
                let u = proto::UpdateWindow {
                    id: w.id,
                    window_data: Some(w.data.text()),
                    inventory: w.section(),
                    extra_resources: extra,
                };
                if let Ok(p) = packet(self::id::UPDATE_WINDOW, &u) {
                    out.to.push((pid, p));
                }
            }
        }
    }

    /// Takes one unit of a queued craft's inputs; what it took.
    fn take_job_unit(
        &self,
        e: &mut Entry,
        job: &crate::crafting::Job,
    ) -> Option<Vec<(String, i32)>> {
        if !job.from_input {
            let before = e.inv.clone();
            let t = self.book.take_unit(&mut e.inv, &job.recipe)?;
            let changed = e.inv.changed_since(&before);
            e.inv_dirty.extend(changed);
            return Some(t);
        }
        let m = self.book.single_input(&job.recipe)?.clone();
        let w = e.window.as_mut()?;
        let input = w.input.clone()?;
        let (item, have) = (input.item.clone(), input.n);
        if !self.book.is_material(&m, &item) || have < m.quantity {
            return None;
        }
        w.input = (have > m.quantity).then(|| input.with_n(have - m.quantity));
        self.update_options(w);
        Some(vec![(item, m.quantity)])
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
                    Some(self.book.stack(&m.item.item_id, m.item.quantity)),
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
        // Items that no longer exist (or never did) are left behind.
        let text: String = text
            .lines()
            .filter(|l| {
                l.split_whitespace()
                    .nth(2)
                    .is_none_or(|i| self.book.known(i))
            })
            .map(|l| format!("{l}\n"))
            .collect();
        Some(Inventory::from_text(&text, |i| self.book.max_durability(i)))
    }

    /// The active hotbar, utility and tools slots a player left with
    /// (kept with the inventory, as on the official server).
    fn saved_slots(&self, uuid: uuid::Uuid) -> Option<[i32; 3]> {
        let file = self.inventory_file(uuid)?.with_extension("slots");
        let text = std::fs::read_to_string(file).ok()?;
        let v: Vec<i32> = text
            .split_whitespace()
            .filter_map(|w| w.parse().ok())
            .collect();
        match v[..] {
            [h, u, t] if (0..9).contains(&h) && u >= -1 && t >= -1 => Some([h, u, t]),
            _ => None,
        }
    }

    fn save_slots(&self, uuid: uuid::Uuid, slots: [i32; 3]) {
        let Some(f) = self.inventory_file(uuid) else {
            return;
        };
        let r = std::fs::create_dir_all(f.parent().unwrap()).and_then(|_| {
            std::fs::write(
                f.with_extension("slots"),
                format!("{} {} {}\n", slots[0], slots[1], slots[2]),
            )
        });
        if let Err(e) = r {
            tracing::warn!("saving active slots failed: {e}");
        }
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

    /// The player's whole inventory as saved (empty for a new player, as
    /// on the official server), sent at once.
    pub fn send_inventory(&self, id: i32) -> Result<()> {
        let p = {
            let mut g = self.inner.lock().unwrap();
            let Some(e) = g.players.get_mut(&id) else {
                return Ok(());
            };
            // The official server fills the tools after it registered the
            // section's change listener, so the next tick sends the tools
            // once more (spec 14).
            e.inv_dirty.insert(inventory::TOOLS);
            packet(id::UPDATE_PLAYER_INVENTORY, &e.inv.packet())?
        };
        self.send(id, p);
        Ok(())
    }

    /// The player's active slots (hotbar, utility, tools), as the official
    /// server tells them right after the inventory at join: chains name
    /// them and are refused when they differ (spec 18).
    pub fn send_active_slots(&self, id: i32) -> Result<()> {
        let slots = {
            let g = self.inner.lock().unwrap();
            let Some(e) = g.players.get(&id) else {
                return Ok(());
            };
            [
                (inventory::HOTBAR, e.hotbar_slot),
                (inventory::UTILITY, e.utility_slot),
                (-8, e.tools_slot),
            ]
        };
        for (section, slot) in slots {
            self.send(
                id,
                packet(
                    id::SET_ACTIVE_SLOT,
                    &proto::SetActiveSlot {
                        inventory_section_id: section,
                        active_slot: slot,
                    },
                )?,
            );
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
        self.world_step(|hub, g, ground, out| {
            hub.change(g, ground, (x, y, z), block, 0, How::default(), out);
        });
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
                let (x, y, z, key, rot, filler) = match w[..] {
                    [x, y, z, key] => (x, y, z, key, 0, 0),
                    [x, y, z, key, r] => (x, y, z, key, r.parse().unwrap_or(0), 0),
                    [x, y, z, key, r, f] => {
                        (x, y, z, key, r.parse().unwrap_or(0), f.parse().unwrap_or(0))
                    }
                    _ => continue,
                };
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
                    if rot != 0 {
                        g.rotations.insert((x, y, z), rot);
                    } else {
                        g.rotations.remove(&(x, y, z));
                    }
                    if filler != 0 {
                        g.fillers.insert((x, y, z), filler);
                    } else {
                        g.fillers.remove(&(x, y, z));
                    }
                    n += 1;
                }
            }
            drop(g);
            info!("loaded {n} block changes from {}", file.display());
        }
        if let Ok(text) = std::fs::read_to_string(dir.join("block-state.txt")) {
            let mut g = self.inner.lock().unwrap();
            let mut at = None;
            let mut chest: Option<Pos> = None;
            for l in text.lines() {
                let w: Vec<&str> = l.split_whitespace().collect();
                if let ["@", x, y, z, "container", n] = w[..] {
                    at = None;
                    chest = match (x.parse(), y.parse(), z.parse()) {
                        (Ok(x), Ok(y), Ok(z)) => Some((x, y, z)),
                        _ => None,
                    };
                    if let Some(c) = chest {
                        g.containers.insert(c, vec![None; n.parse().unwrap_or(0)]);
                    }
                } else if let (Some(c), ["slot", i, item, n, d, m]) = (chest, &w[..]) {
                    if let (Ok(i), Ok(n), Some(slots)) =
                        (i.parse::<usize>(), n.parse(), g.containers.get_mut(&c))
                        && i < slots.len()
                    {
                        slots[i] = Some(Stack {
                            item: (*item).to_owned(),
                            n,
                            durability: d.parse().unwrap_or(0.0),
                            max: m.parse().unwrap_or(0.0),
                        });
                    }
                } else if l.is_empty() {
                    chest = None;
                    at = None;
                } else if let ["@", x, y, z, "processing"] = w[..] {
                    chest = None;
                    at = match (x.parse(), y.parse(), z.parse()) {
                        (Ok(x), Ok(y), Ok(z)) => Some((x, y, z)),
                        _ => None,
                    };
                } else if l.is_empty() {
                    at = None;
                } else if let Some(at) = at {
                    let e: &mut String = g.saved_states.entry(at).or_default();
                    e.push_str(l);
                    e.push('\n');
                }
            }
            g.states_written = text;
        }
        if let Ok(text) = std::fs::read_to_string(dir.join("support.txt")) {
            let mut g = self.inner.lock().unwrap();
            for line in text.lines() {
                let w: Vec<&str> = line.split_whitespace().collect();
                if let [x, y, z, v] = w[..]
                    && let (Ok(x), Ok(y), Ok(z), Ok(v)) =
                        (x.parse(), y.parse(), z.parse(), v.parse::<u8>())
                {
                    if v == 0 {
                        g.support.remove(&(x, y, z));
                    } else {
                        g.support.insert((x, y, z), v);
                    }
                }
            }
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

    fn save_change(&self, x: i32, y: i32, z: i32, block: i32, rot: i32, filler: i32) {
        let Some(file) = &self.save else { return };
        let key = if block == 0 {
            "air".to_owned()
        } else {
            match self.keys_by_id.get(&block) {
                Some(k) => k.clone(),
                None => return,
            }
        };
        let line = if filler != 0 {
            format!("{x} {y} {z} {key} {rot} {filler}")
        } else if rot != 0 {
            format!("{x} {y} {z} {key} {rot}")
        } else {
            format!("{x} {y} {z} {key}")
        };
        append_line(file, &line);
    }

    /// The state blocks carry (spec 20), written whole to `block-state.txt`
    /// next to `blocks.txt` when it changed: `@ x y z kind`, the state's
    /// lines, an empty line.
    fn save_states(&self, g: &mut Inner) {
        let Some(file) = &self.save else { return };
        // What players carry, so that a server that stops without
        // warning loses a few seconds at most.
        for e in g.players.values_mut() {
            let slots = [e.hotbar_slot, e.utility_slot, e.tools_slot];
            let text = e.inv.to_text();
            if e.saved_carry.as_ref() != Some(&(text.clone(), slots)) {
                self.save_inventory(e.uuid, &e.inv);
                self.save_slots(e.uuid, slots);
                e.saved_carry = Some((text, slots));
            }
        }
        let mut text = String::new();
        for (at, st) in &g.processing {
            text += &format!(
                "@ {} {} {} processing\n{}\n",
                at.0,
                at.1,
                at.2,
                st.to_text()
            );
        }
        for (at, st) in &g.saved_states {
            text += &format!("@ {} {} {} processing\n{}\n", at.0, at.1, at.2, st);
        }
        for (at, slots) in &g.containers {
            if slots.iter().all(Option::is_none) {
                continue;
            }
            text += &format!("@ {} {} {} container {}\n", at.0, at.1, at.2, slots.len());
            for (i, st) in slots.iter().enumerate() {
                if let Some(st) = st {
                    text += &format!(
                        "slot {i} {} {} {} {}\n",
                        st.item, st.n, st.durability, st.max
                    );
                }
            }
            text += "\n";
        }
        if text == g.states_written {
            return;
        }
        let path = file.with_file_name("block-state.txt");
        let tmp = file.with_file_name("block-state.txt.tmp");
        match std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, &path)) {
            Ok(()) => g.states_written = text,
            Err(e) => warn!("saving to {} failed: {e}", path.display()),
        }
    }

    /// Support values that are not 0 (decoration), in `support.txt` next
    /// to `blocks.txt`.
    fn save_support(&self, x: i32, y: i32, z: i32, v: u8) {
        if let Some(file) = &self.save {
            append_line(
                &file.with_file_name("support.txt"),
                &format!("{x} {y} {z} {v}"),
            );
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
        if let Some(ms) = &m.movement_states {
            // A sprint that ends holds stamina regeneration back (spec 17).
            if e.sprinting
                && !ms.sprinting
                && let Some((i, v)) = self.stat_types.sprint_delay
                && e.stats.get(i).is_some_and(|cur| cur > v)
            {
                e.stats
                    .set(&self.stat_types, i, v, crate::stats::Predictable::None);
            }
            e.sprinting = ms.sprinting;
            e.gliding = ms.gliding;
            e.movement = ms.clone();
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
        Ok(())
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
        // Its own client sees it once more as the entity tracker shows it
        // (spec 17): model, name plate, hitbox collision, equipment, UI
        // components, every stat, effects.
        {
            use proto::ComponentUpdate as C;
            let all = e.stats.init(&self.stat_types, true);
            let rank = |c: &C| match c {
                C::ModelUpdate(_) => Some(0),
                C::NameplateUpdate(_) => Some(1),
                C::HitboxCollisionUpdate(_) => Some(2),
                C::EquipmentUpdate(_) => Some(3),
                C::UIComponentsUpdate(_) => Some(4),
                C::EntityStatsUpdate(_) => Some(5),
                C::EntityEffectsUpdate(_) => Some(6),
                _ => None,
            };
            let mut parts: Vec<(i32, C)> = view
                .updates
                .iter()
                .flatten()
                .filter_map(|c| Some((rank(c)?, c.clone())))
                .collect();
            parts.sort_by_key(|(r, _)| *r);
            let parts: Vec<C> = parts
                .into_iter()
                .map(|(_, c)| match c {
                    C::EntityStatsUpdate(_) => C::EntityStatsUpdate(Box::new(all.clone())),
                    c => c,
                })
                .collect();
            if let Ok(p) = packet(
                id::ENTITY_UPDATES,
                &proto::EntityUpdates {
                    removed: None,
                    updates: Some(vec![proto::EntityUpdate {
                        network_id: id,
                        removed: None,
                        updates: Some(parts),
                    }]),
                },
            ) {
                let _ = e.outbox.send(p);
            }
        }
        // Others see its shared stats only, at their current values.
        let mut view = view;
        let shared = e.stats.init(&self.stat_types, false);
        for c in view.updates.iter_mut().flatten() {
            if let proto::ComponentUpdate::EntityStatsUpdate(s) = c {
                **s = shared.clone();
            }
        }
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
                // Where it is now, with its stats as they are.
                let stats = o.stats.init(&self.stat_types, false);
                for c in v.updates.iter_mut().flatten() {
                    if let proto::ComponentUpdate::EntityStatsUpdate(s) = c {
                        **s = stats.clone();
                    }
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
        // Blocks falling right now.
        theirs.extend(
            g.falling
                .iter()
                .filter(|(_, f)| f.announced)
                .map(|(n, f)| falling_entity(*n, f)),
        );
        // Items lying around.
        theirs.extend(
            g.items
                .iter()
                .filter(|(_, d)| d.announced)
                .map(|(n, d)| item_entity(*n, &d.stack(), d.pos)),
        );
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
    pub fn interactions(&self, _player: i32, payload: &[u8]) -> Result<()> {
        let (m, _) = proto::SyncInteractionChains::decode(payload, 0)?;
        if !self.chains.roots.is_empty() {
            // Chains run on the server (spec 18): queued for the tick.
            self.world_step(|hub, g, ground, out| {
                let Some(mut manager) = g
                    .players
                    .get_mut(&_player)
                    .map(|e| std::mem::take(&mut e.chains))
                else {
                    return;
                };
                manager.receive(
                    m.updates,
                    &mut ChainHost {
                        hub,
                        g,
                        ground,
                        out,
                        pid: _player,
                    },
                );
                if let Some(e) = g.players.get_mut(&_player) {
                    e.chains = manager;
                }
            });
            return Ok(());
        }
        // Without the interaction registries (tests): the direct path.
        let mut placements = Vec::new();
        let mut hits = Vec::new();
        let mut uses = Vec::new();
        fn fork_path(f: Option<&proto::ForkedChainId>) -> String {
            let mut s = String::new();
            let mut f = f;
            while let Some(x) = f {
                s += &format!("{}.{}/", x.entry_index, x.sub_index);
                f = x.forked_id.as_deref();
            }
            s
        }
        let mut walk = |c: &proto::SyncInteractionChain| -> Result<()> {
            if c.initial
                && c.forked_id.is_none()
                && c.interaction_type == proto::InteractionType::PRIMARY
            {
                if let Some(t) = c.data.as_ref().and_then(|d| d.block_position.as_ref()) {
                    hits.push(((t.x, t.y, t.z), c.active_hotbar_slot));
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
            // A placement operation: a block, its rotation and the block
            // the client put (spec 15).
            for (i, d) in c.interaction_data.iter().flatten().enumerate() {
                let Some(d) = d else { continue };
                if let (Some(p), Some(r), true) = (
                    &d.block_position,
                    &d.block_rotation,
                    d.placed_block_id != i32::MIN,
                ) {
                    let rot = r.rotation_roll.0 * 16 + r.rotation_pitch.0 * 4 + r.rotation_yaw.0;
                    let op = (
                        c.chain_id,
                        fork_path(c.forked_id.as_ref()),
                        c.operation_base_index + i as i32,
                    );
                    placements.push((
                        op,
                        Placement {
                            pos: (p.x, p.y, p.z),
                            rot,
                            client_block: d.placed_block_id,
                            slot: c.active_hotbar_slot,
                        },
                    ));
                }
            }
            Ok(())
        };
        // The hotbar slot the client's chains name (until chains run on
        // the server, the held item follows the client).
        if let Some(c) = m.updates.first()
            && let Some(e) = self.inner.lock().unwrap().players.get_mut(&_player)
            && (0..9).contains(&c.active_hotbar_slot)
        {
            e.hotbar_slot = c.active_hotbar_slot;
        }
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
        for ((x, y, z), slot) in hits {
            self.hit(_player, x, y, z, slot);
        }
        for (op, p) in placements {
            let first = {
                let mut g = self.inner.lock().unwrap();
                g.players.get_mut(&_player).is_some_and(|e| {
                    if e.placed_ops.len() > 4096 {
                        e.placed_ops.clear();
                    }
                    e.placed_ops.insert(op)
                })
            };
            if first {
                self.place(_player, p);
            }
        }
        Ok(())
    }

    /// A chat line from a translation key (`server.modules.interaction.*`).
    fn tell_key(&self, pid: i32, key: &str, out: &mut Outgoing) {
        if let Ok(p) = packet(
            id::SERVER_MESSAGE,
            &proto::ServerMessage {
                r#type: proto::ChatType::CHAT,
                message: Some(proto::FormattedMessage {
                    message_id: Some(key.to_owned()),
                    ..Default::default()
                }),
            },
        ) {
            out.to.push((pid, p));
        }
    }

    /// The client is sent the block a position really holds (the official
    /// `invalidateBlock`): it goes out with the tick's changes.
    fn resend_block(g: &mut Inner, (x, y, z): Pos) {
        let (sec, index) = section_of(x, y, z);
        let tick = g.tick;
        g.sections.entry(sec).or_default().add(tick, index);
        g.touched.insert(sec);
    }

    /// A player places a block (spec 15), as the official place
    /// interaction: the held stack, reach, the block it may become, the
    /// item used up in adventure, free cells (soft blocks there break with
    /// their drops), decoration marking; a failed placement gives the item
    /// back and resends the block.
    fn place(&self, pid: i32, p: Placement) {
        self.world_step(|hub, g, ground, out| hub.place_in(g, ground, out, pid, p));
    }

    /// [`Hub::place`] inside a world step.
    fn place_in(&self, g: &mut Inner, ground: &Ground, out: &mut Outgoing, pid: i32, p: Placement) {
        let hub = self;
        {
            use crate::support::World;
            let Some(e) = g.players.get(&pid) else { return };
            let creative = e.creative;
            let no_physics = creative && e.no_physics;
            let Some(held) = e.inv.get(inventory::HOTBAR, p.slot) else {
                return;
            };
            let Some(at) = e.position.clone() else { return };
            let mut reach = hub.book.use_distance(Some(&held.item), creative);
            if creative {
                reach = reach.max(e.creative_distance.clamp(0, 128) as f32);
            }
            reach += 2.0;
            let (x, y, z) = p.pos;
            let d = [
                x as f64 + 0.5 - at.x,
                y as f64 + 0.5 - (at.y + hub.eye_height),
                z as f64 + 0.5 - at.z,
            ];
            if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > (reach * reach) as f64 {
                return;
            }
            let item_block = hub.book.block_of(&held.item);
            if item_block <= 0 {
                return;
            }
            let mut client = (p.client_block != -1).then_some(p.client_block);
            if let Some(c) = client {
                if hub.rules.block(c).is_none() {
                    return;
                }
                if !no_physics
                    && c != item_block
                    && !hub
                        .rules
                        .block(item_block)
                        .is_some_and(|b| b.place_overrides.contains(&c))
                {
                    client = None;
                }
            }
            let block = client.unwrap_or(item_block);
            // Used up in adventure.
            let mut taken = false;
            if !creative {
                let e = g.players.get_mut(&pid).unwrap();
                match e.inv.get(inventory::HOTBAR, p.slot) {
                    Some(s) if s.stacks_with(&held) && s.n >= 1 => {
                        e.inv
                            .set(inventory::HOTBAR, p.slot, Some(s.with_n(s.n - 1)));
                        e.inv_dirty.insert(inventory::HOTBAR);
                        taken = true;
                    }
                    _ => {
                        hub.tell_key(pid, "server.modules.interaction.failedCheck", out);
                        return;
                    }
                }
            }
            let fail = |g: &mut Inner, out: &mut Outgoing| {
                if taken && let Some(e) = g.players.get_mut(&pid) {
                    let one = held.with_n(1);
                    let back = match e.inv.get(inventory::HOTBAR, p.slot) {
                        None => {
                            e.inv.set(inventory::HOTBAR, p.slot, Some(one.clone()));
                            0
                        }
                        Some(s) if s.stacks_with(&one) && s.n < hub.book.max_stack(&one.item) => {
                            e.inv
                                .set(inventory::HOTBAR, p.slot, Some(s.with_n(s.n + 1)));
                            0
                        }
                        _ => e.inv.add(&one, hub.book.max_stack(&one.item)),
                    };
                    e.inv_dirty.insert(inventory::HOTBAR);
                    if back > 0 {
                        hub.tell_key(
                            pid,
                            "server.modules.interaction.failedAddBackAfterFailedPlace",
                            out,
                        );
                        return;
                    }
                }
                Self::resend_block(g, p.pos);
            };
            let Some(b) = hub.rules.block(block) else {
                hub.tell_key(pid, "server.modules.interaction.failedCheckBlock", out);
                return;
            };
            if block == 0 {
                hub.tell_key(pid, "server.modules.interaction.failedCheckEmpty", out);
                return;
            }
            if b.unknown {
                hub.tell_key(pid, "server.modules.interaction.failedCheckUnknown", out);
                return;
            }
            let cells = hub.rules.cells(block, p.rot);
            // Every cell free: air, a soft (non-solid) block, or an unknown
            // filler.
            let view = |g: &Inner, c: Pos| {
                View {
                    g,
                    ground,
                    hub_rules: &hub.rules,
                }
                .block(c.0, c.1, c.2)
            };
            if !no_physics {
                let free = cells.iter().all(|c| {
                    let at = (x + c[0], y + c[1], z + c[2]);
                    if !(0..crate::support::WORLD_HEIGHT).contains(&at.1) {
                        return false;
                    }
                    let (other, _) = view(g, at);
                    let filler = g.fillers.get(&at).copied().unwrap_or(0);
                    match hub.rules.block(other) {
                        None => false,
                        Some(_) if other == 0 => true,
                        Some(o) if !o.solid => true,
                        Some(o) => filler != 0 && o.unknown,
                    }
                });
                if !free {
                    fail(g, out);
                    return;
                }
            }
            // Soft blocks in the way break with their drops.
            for c in &cells {
                let at = (x + c[0], y + c[1], z + c[2]);
                let (other, _) = view(g, at);
                if other == 0 || hub.rules.block(other).is_none_or(|o| o.solid) {
                    continue;
                }
                let drop = hub.rules.block(other).and_then(|o| o.soft_drop.clone());
                let drops = drop.map_or((None, None, 1), |d| (d.item, d.list, 1));
                hub.break_with(g, ground, at, Some(drops), true, !no_physics, out);
            }
            let how = How {
                physics: !no_physics,
                ..Default::default()
            };
            let mark = out.all.len();
            let changed = hub.change(g, ground, p.pos, block, p.rot, how, out);
            let (later, now): (Vec<Packet>, Vec<Packet>) = out
                .all
                .drain(mark..)
                .partition(|(i, _)| *i == id::SPAWN_BLOCK_PARTICLE_SYSTEM);
            out.all.extend(now);
            g.deferred_particles.extend(later);
            if changed.is_none() {
                fail(g, out);
                return;
            }
            hub.connect(g, ground, p.pos, true, out);
            if no_physics || b.deco_when_placed {
                g.support.insert(p.pos, crate::support::DECO);
                hub.save_support(x, y, z, crate::support::DECO);
            }
        }
    }

    /// Damaged blocks of a column, per section, as the official server
    /// sends them when a client loads the chunk (`UpdateBlockDamages`, delta
    /// 0).
    pub fn damage_packets(&self, cx: i32, cz: i32) -> Vec<Packet> {
        let g = self.inner.lock().unwrap();
        let mut by_section: BTreeMap<Pos, Vec<proto::BlockDamageCmd>> = BTreeMap::new();
        for (&(x, y, z), &(h, _)) in &g.health {
            if x.div_euclid(32) != cx || z.div_euclid(32) != cz {
                continue;
            }
            let (sec, index) = section_of(x, y, z);
            by_section
                .entry(sec)
                .or_default()
                .push(proto::BlockDamageCmd {
                    index: index as i16,
                    damage: h,
                    delta: 0.0,
                });
        }
        by_section
            .into_iter()
            .filter_map(|(sec, mut cmds)| {
                cmds.sort_by_key(|c| c.index);
                packet(
                    id::UPDATE_BLOCK_DAMAGES,
                    &proto::UpdateBlockDamages {
                        x: sec.0,
                        y: sec.1,
                        z: sec.2,
                        cmds,
                    },
                )
                .ok()
            })
            .collect()
    }

    /// The changed blocks inside section (cx, cy, cz), as (x, y, z within
    /// the section, block id).
    pub fn changes_in(
        &self,
        cx: i32,
        cy: i32,
        cz: i32,
    ) -> Vec<(usize, usize, usize, i32, i32, i32)> {
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
                    g.rotations.get(&(x, y, z)).copied().unwrap_or(0),
                    g.fillers.get(&(x, y, z)).copied().unwrap_or(0),
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
                 /blocks <part of a name>, /give <item> [amount], /clear, /perf",
            ),
            ("perf", _) => {
                let on = self.toggle_perf(id);
                self.tell(
                    id,
                    if on {
                        "Performance panel on"
                    } else {
                        "Performance panel off"
                    },
                );
            }
            ("block", ["set", x, y, z, block]) => {
                let here = self.position(id);
                let b = if block.eq_ignore_ascii_case("empty") {
                    Some(0)
                } else {
                    self.block_id(block)
                };
                match (block_coords(here.as_ref(), [x, y, z]), b) {
                    (Some([x, y, z]), Some(b)) => {
                        self.set_block(x, y, z, b)?;
                        self.tell(id, &format!("Set {x} {y} {z} to {block}"));
                    }
                    _ => self.tell(id, "Usage: /block set <x> <y> <z> <block>"),
                }
            }
            ("block", ["setticking", x, y, z]) => {
                let here = self.position(id);
                match block_coords(here.as_ref(), [x, y, z]) {
                    Some([x, y, z]) => {
                        self.set_ticking(x, y, z);
                        self.tell(id, &format!("{x} {y} {z} will be checked"));
                    }
                    None => self.tell(id, "Usage: /block setticking <x> <y> <z>"),
                }
            }
            ("clear", []) => {
                self.with_inventory(id, |inv| *inv = Inventory::default())?;
                self.tell(id, "Inventory cleared");
            }
            // The official form: `/give <player> <item> [--quantity=N]`,
            // into the target's pickup sections, with the official reply.
            ("give", [who, item, rest @ ..])
                if who.starts_with('@')
                    || self.names().iter().any(|n| n.eq_ignore_ascii_case(who)) =>
            {
                // `@s` and the like: the player who typed it.
                let own = who.starts_with('@');
                let qty = rest
                    .iter()
                    .find_map(|a| a.strip_prefix("--quantity="))
                    .and_then(|q| q.parse::<i32>().ok())
                    .unwrap_or(1);
                let key = self
                    .names
                    .get(&item.to_lowercase())
                    .cloned()
                    .unwrap_or_else(|| item.to_string());
                if !self.book.known(&key) {
                    self.tell(id, &format!("No item is called {item}"));
                    return Ok(());
                }
                let left = {
                    let mut g = self.inner.lock().unwrap();
                    let Some((_, e)) = g.players.iter_mut().find(|(pid, e)| {
                        if own {
                            **pid == id
                        } else {
                            e.name.eq_ignore_ascii_case(who)
                        }
                    }) else {
                        return Ok(());
                    };
                    let items = ItemsView {
                        book: &self.book,
                        rules: &self.rules,
                        bench: None,
                        processing: None,
                        container: false,
                    };
                    let order = e.prefs.order_for(inventory::Items::kind(&items, &key));
                    let before = e.inv.clone();
                    let left = e.inv.add_to(&items, &order, &self.book.stack(&key, qty));
                    let changed = e.inv.changed_since(&before);
                    e.inv_dirty.extend(changed);
                    left
                };
                let name = proto::FormattedMessage {
                    message_id: Some(self.book.item_name(&key)),
                    ..Default::default()
                };
                let int = |v: i32| {
                    proto::ParamValue::IntParamValue(Box::new(proto::IntParamValue { value: v }))
                };
                let msg = if left > 0 {
                    proto::FormattedMessage {
                        message_id: Some("server.commands.give.insufficientInvSpace".into()),
                        params: Some(vec![("quantity".into(), int(qty))]),
                        message_params: Some(vec![("item".into(), name)]),
                        ..Default::default()
                    }
                } else {
                    proto::FormattedMessage {
                        message_id: Some("server.commands.give.gave".into()),
                        params: Some(vec![
                            (
                                "targetUsername".into(),
                                proto::ParamValue::StringParamValue(Box::new(
                                    proto::StringParamValue {
                                        value: Some((*who).to_owned()),
                                    },
                                )),
                            ),
                            ("quantity".into(), int(qty)),
                        ]),
                        message_params: Some(vec![("item".into(), name)]),
                        ..Default::default()
                    }
                };
                self.send(
                    id,
                    packet(
                        id::SERVER_MESSAGE,
                        &proto::ServerMessage {
                            r#type: proto::ChatType::CHAT,
                            message: Some(msg),
                        },
                    )?,
                );
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
                if !self.book.known(&key) {
                    self.tell(id, &format!("No item is called {item}"));
                    return Ok(());
                }
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
            ("player", ["stats", rest @ ..]) => self.stats_command(id, rest)?,
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
/// How clients first see an item entity: its stack (entity scale 0),
/// intangible, its position.
fn item_entity(nid: i32, st: &Stack, pos: [f64; 3]) -> proto::EntityUpdate {
    proto::EntityUpdate {
        network_id: nid,
        removed: None,
        updates: Some(vec![
            item_update(st),
            proto::ComponentUpdate::IntangibleUpdate(Box::default()),
            transform_update(pos),
        ]),
    }
}

/// A falling block as clients first hear of it (spec 24).
fn falling_entity(nid: i32, f: &crate::falling::Falling) -> proto::EntityUpdate {
    let mut updates = vec![proto::ComponentUpdate::BlockUpdate(Box::new(
        proto::BlockUpdate {
            block_id: f.block,
            entity_scale: 1.0,
        },
    ))];
    if f.collision >= 0 {
        updates.push(proto::ComponentUpdate::HitboxCollisionUpdate(Box::new(
            proto::HitboxCollisionUpdate {
                hitbox_collision_config_index: f.collision,
            },
        )));
    }
    updates.push(falling_transform(f));
    proto::EntityUpdate {
        network_id: nid,
        removed: None,
        updates: Some(updates),
    }
}

/// Where a falling block is: its block's rotation turned half around,
/// and a level look.
fn falling_transform(f: &crate::falling::Falling) -> proto::ComponentUpdate {
    let (yaw, pitch, roll) = crate::support::rotation(f.rot);
    let quarter = |n: i32| (n as f64 * 90.0).to_radians() as f32;
    proto::ComponentUpdate::TransformUpdate(Box::new(proto::TransformUpdate {
        transform: proto::ModelTransform {
            position: Some(proto::Position {
                x: f.pos[0],
                y: f.pos[1],
                z: f.pos[2],
            }),
            body_orientation: Some(proto::Direction {
                yaw: quarter(yaw) + std::f32::consts::PI,
                pitch: quarter(pitch),
                roll: quarter(roll),
            }),
            look_orientation: Some(proto::Direction {
                yaw: 0.0,
                pitch: 0.0,
                roll: 0.0,
            }),
        },
    }))
}

fn item_update(st: &Stack) -> proto::ComponentUpdate {
    proto::ComponentUpdate::ItemUpdate(Box::new(proto::ItemUpdate {
        item: st.packet(),
        entity_scale: 0.0,
    }))
}

fn transform_update(pos: [f64; 3]) -> proto::ComponentUpdate {
    proto::ComponentUpdate::TransformUpdate(Box::new(proto::TransformUpdate {
        transform: proto::ModelTransform {
            position: Some(proto::Position {
                x: pos[0],
                y: pos[1],
                z: pos[2],
            }),
            body_orientation: None,
            look_orientation: None,
        },
    }))
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
        // The particle at once; the block at the end of the tick.
        assert_eq!(texts(&mut ra), ["#153", "Set 1 2 3 to Rock_Stone"]);
        hub.tick();
        // The block, the column's new height, then the network tick.
        assert_eq!(texts(&mut ra), ["#140", "#183", "#5"]);
        hub.chat(a, &chat("/setblock 1 2 3 Nope")).unwrap();
        assert_eq!(texts(&mut ra), ["No block Nope; try /blocks <name>"]);
        hub.chat(a, &chat("/fill 0 0 0 1 1 0 7")).unwrap();
        assert_eq!(texts(&mut ra).last().unwrap(), "Filled 4 blocks with 7");
        hub.tick();
        // Four changes in one section and tick: one ServerSetBlocks.
        assert_eq!(texts(&mut ra)[0], "#141");
        // Later joiners get them in their chunks.
        assert_eq!(hub.changes_in(0, 0, 0).len(), 5);
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
        hub.send_inventory(a).unwrap();
        let (i, _) = ra.try_recv().unwrap();
        assert_eq!(i, id::UPDATE_PLAYER_INVENTORY);
        assert_eq!(hub.give(a, "Rock_Stone", 3).unwrap(), 0);
        assert_eq!(hub.give(a, "Torch", 5).unwrap(), 1);
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
        // Creative: a hit breaks at once.
        let hub = Hub::new(HashMap::new()).with_creative(true);
        hub.set_ground(vec![1; 64], None);
        let (a, mut ra) = hub.join("Ana", uuid::Uuid::from_u128(1));
        hub.interactions(a, &sync(proto::InteractionType::PRIMARY, true, None))
            .unwrap();
        hub.tick();
        assert_eq!(set_blocks(&mut ra), [(18, 63, 16, 0)]);
        // Holding the button sends more of the same chain: nothing new.
        hub.interactions(a, &sync(proto::InteractionType::PRIMARY, false, None))
            .unwrap();
        hub.tick();
        assert!(set_blocks(&mut ra).is_empty());
        // The client reports a placement, but holds nothing that places
        // that block: the official server places nothing (spec 15).
        hub.interactions(
            a,
            &sync(
                proto::InteractionType::SECONDARY,
                false,
                Some((17, 64, 14, 1530)),
            ),
        )
        .unwrap();
        hub.tick();
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
        hub.interactions(a, &std::fs::read(&f).unwrap()).unwrap();
        hub.tick();
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
        hub.set_block(4, 5, 6, 7).unwrap();
        hub.set_block(4, 5, 6, 0).unwrap();
        let again = Hub::with_names(blocks, names)
            .with_world(dir.clone())
            .unwrap();
        assert_eq!(again.block_at(1, 2, 3), Some(7));
        assert_eq!(again.block_at(4, 5, 6), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn append_line(file: &std::path::Path, line: &str) {
    use std::io::Write;
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = r {
        warn!("saving to {} failed: {e}", file.display());
    }
}

/// The ground under the changes, as the hub keeps it.
type Ground = Option<(Vec<i32>, Option<crate::terrain::Generator>)>;

fn ground_block(ground: &Ground, x: i32, y: i32, z: i32) -> i32 {
    match ground {
        Some((_, Some(g))) => g.block(x, y, z),
        Some((layers, None)) => usize::try_from(y)
            .ok()
            .and_then(|y| layers.get(y))
            .copied()
            .unwrap_or(0),
        None => 0,
    }
}

/// The world as block support sees it.
struct View<'a> {
    g: &'a Inner,
    ground: &'a Ground,
    hub_rules: &'a crate::support::Rules,
}

impl crate::support::World for View<'_> {
    fn block(&self, x: i32, y: i32, z: i32) -> (i32, i32) {
        if !(0..crate::support::WORLD_HEIGHT).contains(&y) {
            return (0, 0);
        }
        let id = match self.g.placed.get(&(x, y, z)) {
            Some(&b) => b,
            None => ground_block(self.ground, x, y, z),
        };
        (id, self.g.rotations.get(&(x, y, z)).copied().unwrap_or(0))
    }
    fn support_value(&self, x: i32, y: i32, z: i32) -> u8 {
        self.g.support.get(&(x, y, z)).copied().unwrap_or(0)
    }
}

impl crate::items::Blocks for View<'_> {
    fn solid_boxes(&self, x: i32, y: i32, z: i32) -> Option<Vec<[f64; 6]>> {
        use crate::support::World;
        let (id, rot) = self.block(x, y, z);
        self.hub_rules.solid_boxes(id, rot)
    }
}

/// What a player's interaction chains see of the world (spec 18).
struct ChainHost<'a> {
    hub: &'a Hub,
    g: &'a mut Inner,
    ground: &'a Ground,
    out: &'a mut Outgoing,
    pid: i32,
}

impl ChainHost<'_> {
    fn player(&self) -> Option<&Entry> {
        self.g.players.get(&self.pid)
    }
}

impl crate::chains::Host for ChainHost<'_> {
    fn creative(&self) -> bool {
        self.player().is_some_and(|e| e.creative)
    }

    fn active_slot(&self, section: i32) -> i32 {
        self.player().map_or(-1, |e| match section {
            inventory::HOTBAR => e.hotbar_slot,
            inventory::UTILITY => e.utility_slot,
            -8 => e.tools_slot,
            _ => -1,
        })
    }

    fn item_at(&self, section: i32, slot: i32) -> Option<String> {
        self.player()?
            .inv
            .get(section, slot)
            .map(|s| s.item)
            .filter(|i| !i.is_empty() && i != "Empty")
    }

    fn movement(&self) -> proto::MovementStates {
        self.player()
            .map(|e| e.movement.clone())
            .unwrap_or_default()
    }

    fn block(&self, (x, y, z): Pos) -> Option<i32> {
        use crate::support::World;
        if !(0..crate::support::WORLD_HEIGHT).contains(&y) {
            return None;
        }
        Some(
            View {
                g: self.g,
                ground: self.ground,
                hub_rules: &self.hub.rules,
            }
            .block(x, y, z)
            .0,
        )
    }

    fn cell(&self, (x, y, z): Pos) -> Option<(i32, proto::BlockRotation)> {
        use crate::support::World;
        if !(0..crate::support::WORLD_HEIGHT).contains(&y) {
            return None;
        }
        let (id, rot) = View {
            g: self.g,
            ground: self.ground,
            hub_rules: &self.hub.rules,
        }
        .block(x, y, z);
        Some((
            id,
            proto::BlockRotation {
                rotation_yaw: proto::Rotation((rot % 4) as _),
                rotation_pitch: proto::Rotation((rot / 4 % 4) as _),
                rotation_roll: proto::Rotation((rot / 16 % 4) as _),
            },
        ))
    }

    fn base_of(&self, (x, y, z): Pos) -> Pos {
        let f = unpack_filler(self.g.fillers.get(&(x, y, z)).copied().unwrap_or(0));
        (x - f[0], y - f[1], z - f[2])
    }

    fn in_reach(&self, held: Option<&str>, (x, y, z): Pos) -> bool {
        let Some(e) = self.player() else { return false };
        let Some(at) = &e.position else { return false };
        let mut reach = self.hub.book.use_distance(held, e.creative);
        if e.creative {
            reach = reach.max(e.creative_distance.clamp(0, 128) as f32);
        }
        reach += 2.0;
        let d = [
            x as f64 + 0.5 - at.x,
            y as f64 + 0.5 - (at.y + self.hub.eye_height),
            z as f64 + 0.5 - at.z,
        ];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= (reach * reach) as f64
    }

    fn place(&mut self, r: &crate::chains::PlaceRequest) -> Option<bool> {
        let e = self.player()?;
        let held = e.inv.get(inventory::HOTBAR, r.slot)?;
        if r.client_block != -1 && self.hub.rules.block(r.client_block).is_none() {
            return None;
        }
        let used_up = !e.creative && held.n == 1 && r.remove_item;
        let rot = r.rotation.rotation_roll.0 as i32 * 16
            + r.rotation.rotation_pitch.0 as i32 * 4
            + r.rotation.rotation_yaw.0 as i32;
        self.hub.place_in(
            self.g,
            self.ground,
            self.out,
            self.pid,
            Placement {
                pos: r.pos,
                rot,
                client_block: r.client_block,
                slot: r.slot,
            },
        );
        Some(used_up)
    }

    fn break_block(
        &mut self,
        pos: Pos,
        ctx: &crate::chains::Ctx,
        harvest: bool,
        _soft_only: bool,
        tool: Option<&str>,
        match_tool: bool,
    ) -> bool {
        if harvest {
            return self
                .hub
                .harvest(self.g, self.ground, self.out, self.pid, pos);
        }
        self.hub.hit_in(
            self.g,
            self.ground,
            self.out,
            self.pid,
            pos,
            ctx.slot,
            Some((tool.map(str::to_owned), match_tool)),
        );
        true
    }

    fn door(&mut self, pos: Pos, horizontal: bool) -> bool {
        self.hub
            .use_door(self.g, self.ground, self.out, self.pid, pos, horizontal)
    }

    fn other(&mut self, ty: &str, ctx: &crate::chains::Ctx) {
        // A block's own interaction that opens something: benches.
        if ty.starts_with("Open")
            && let Some(at) = ctx.target_block
        {
            self.out.uses.push((self.pid, at));
        }
    }

    fn cancel(&mut self, chain: i32, fork: Option<proto::ForkedChainId>) {
        if let Ok(p) = packet(
            id::CANCEL_INTERACTION_CHAIN,
            &proto::CancelInteractionChain {
                chain_id: chain,
                forked_id: fork,
            },
        ) {
            self.out.to.push((self.pid, p));
        }
    }

    fn set_active_slot(&mut self, section: i32, slot: i32) {
        if let Ok(p) = packet(
            id::SET_ACTIVE_SLOT,
            &proto::SetActiveSlot {
                inventory_section_id: section,
                active_slot: slot,
            },
        ) {
            self.out.to.push((self.pid, p));
        }
    }

    fn change_hotbar_slot(&mut self, slot: i32) {
        if (0..9).contains(&slot)
            && let Some(e) = self.g.players.get_mut(&self.pid)
        {
            e.hotbar_slot = slot;
        }
    }

    fn modify_inventory(
        &mut self,
        m: &proto::ModifyInventoryInteraction,
        ctx: &mut crate::chains::Ctx,
    ) -> bool {
        let Some(e) = self.g.players.get_mut(&self.pid) else {
            return false;
        };
        if m.required_game_mode
            .is_some_and(|g| (g == proto::GameMode::CREATIVE) != e.creative)
        {
            return true;
        }

        let book = &self.hub.book;
        let add = |e: &mut Entry, item: &str, n: i32| {
            let st = book.stack(item, n);
            let max = book.max_stack(item);
            e.inv.add(&st, max);
        };
        let before = e.inv.clone();
        let mut ok = true;
        'run: {
            // An item taken from the hotbar, storage or backpack: all of
            // it or nothing.
            if let Some(r) = &m.item_to_remove {
                let have: i32 = e
                    .inv
                    .all()
                    .iter()
                    .filter(|(i, _)| *i == r.item_id)
                    .map(|(_, n)| *n)
                    .sum();
                if have < r.quantity
                    || e.inv
                        .take_units(|i| (i == r.item_id).then_some(1), r.quantity)
                        > 0
                {
                    ok = false;
                    break 'run;
                }
            }
            let held = e
                .inv
                .get(ctx.section, ctx.slot)
                .filter(|s| ctx.held.as_deref() == Some(s.item.as_str()));
            if let Some(h) = &held {
                let n = m.adjust_held_item_quantity;
                if n < 0 {
                    if h.n < -n {
                        ok = false;
                        break 'run;
                    }
                    let left = (h.n + n > 0).then(|| h.with_n(h.n + n));
                    ctx.held = left.as_ref().map(|s| s.item.clone());
                    e.inv.set(ctx.section, ctx.slot, left);
                } else if n > 0 {
                    add(e, &h.item, n);
                }
            }
            if let Some(a) = &m.item_to_add {
                add(e, &a.item_id, a.quantity);
            }
            if m.adjust_held_item_durability != 0.0
                && let Some(h) = e
                    .inv
                    .get(ctx.section, ctx.slot)
                    .filter(|s| ctx.held.as_deref() == Some(s.item.as_str()))
            {
                let mut new = Some(h.worn(m.adjust_held_item_durability));
                let just_broke = new.as_ref().is_some_and(Stack::broken) && !h.broken();
                let transforms = m.broken_item.as_ref().is_some_and(|b| *b != h.item);
                if new.as_ref().is_some_and(Stack::broken)
                    && let Some(b) = &m.broken_item
                {
                    if b == "Empty" {
                        new = None;
                    } else if *b != h.item {
                        new = Some(book.stack(b, 1));
                    }
                }
                ctx.held = new.as_ref().map(|s| s.item.clone());
                e.inv.set(ctx.section, ctx.slot, new);
                if just_broke && !transforms {
                    self.hub.tell_broken(self.pid, &h, self.out);
                }
            }
        }
        let Some(e) = self.g.players.get_mut(&self.pid) else {
            return false;
        };
        let changed = e.inv.changed_since(&before);
        e.inv_dirty.extend(changed);
        ok
    }

    fn change_stats(
        &mut self,
        stats: &[(i32, f32)],
        percent: bool,
        behaviour: proto::ChangeStatBehaviour,
    ) {
        use crate::stats::Predictable::Own;
        let types = &self.hub.stat_types;
        let Some(e) = self.g.players.get_mut(&self.pid) else {
            return;
        };
        for &(i, amount) in stats {
            let Ok(i) = usize::try_from(i) else { continue };
            let (Some(cur), Some(max)) = (e.stats.get(i), e.stats.max(i)) else {
                continue;
            };
            let amount = if percent {
                amount * (max - e.stats.min(i).unwrap_or(0.0)) / 100.0
            } else {
                amount
            };
            match behaviour {
                proto::ChangeStatBehaviour::SET => e.stats.set(types, i, amount, Own),
                proto::ChangeStatBehaviour::MIN => e.stats.set(types, i, cur.min(amount), Own),
                proto::ChangeStatBehaviour::MAX => e.stats.set(types, i, cur.max(amount), Own),
                _ => e.stats.add(types, i, amount, Own),
            };
        }
    }

    fn apply_effect(&mut self, effect: i32, duration: Option<f32>) {
        let (book, types) = (&self.hub.effects, &self.hub.stat_types);
        let Some(e) = self.g.players.get_mut(&self.pid) else {
            return;
        };
        let who = StatWho {
            creative: e.creative,
            sprinting: e.sprinting,
            gliding: e.gliding,
        };
        let ok = book
            .get(effect)
            .is_some_and(|x| e.stats.met(types, &who, &x.conditions));
        if e.effects.add(book, effect, duration, ok) {
            e.mods_dirty = true;
        }
    }

    fn has_effect(&self, effect: i32) -> bool {
        self.player().is_some_and(|e| e.effects.has(effect))
    }

    fn clear_effect(&mut self, effect: i32) {
        let Some(how) = self.hub.effects.get(effect).map(|x| x.removal) else {
            return;
        };
        if let Some(e) = self.g.players.get_mut(&self.pid) {
            e.effects.remove(effect, how);
            e.mods_dirty = true;
        }
    }

    fn stat(&self, index: i32) -> Option<(f32, f32, f32)> {
        let e = self.player()?;
        let i = usize::try_from(index).ok()?;
        Some((e.stats.get(i)?, e.stats.min(i)?, e.stats.max(i)?))
    }

    fn move_item(&mut self, m: &crate::chains::Move) {
        self.hub
            .inventory_move_in(self.g, self.pid, |inv, items, _, _| {
                inv.move_to_slot(items, m.from, m.quantity, m.to);
            });
        if let Some(e) = self.g.players.get(&self.pid)
            && let Ok(p) = packet(id::UPDATE_PLAYER_INVENTORY, &e.inv.packet())
        {
            self.out.to.push((self.pid, p));
        }
    }

    fn play(&mut self, p: proto::PlayInteractionFor, view_distance: f64, owner_too: bool) {
        let Some(at) = self.player().and_then(|e| e.position.clone()) else {
            return;
        };
        let Ok(pk) = packet(id::PLAY_INTERACTION_FOR, &p) else {
            return;
        };
        for (id, e) in &self.g.players {
            if *id == self.pid && !owner_too {
                continue;
            }
            let near = e.position.as_ref().is_some_and(|o| {
                let d = [o.x - at.x, o.y - at.y, o.z - at.z];
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= view_distance * view_distance
            });
            if near {
                self.out.to.push((*id, pk.clone()));
            }
        }
    }

    fn network_id(&self) -> i32 {
        self.pid
    }
}

/// Packets and drops a world step produces, sent once the locks are gone.
#[derive(Default)]
struct Outgoing {
    all: Vec<Packet>,
    to: Vec<(i32, Packet)>,
    /// Packets for one player that keep their place among `all`: how
    /// many of those were queued before each.
    placed: Vec<(usize, i32, Packet)>,
    drops: Vec<(Vec<(String, i32)>, [f64; 3])>,
    /// Stacks a player drops (speed 1, spec 11).
    throws: Vec<(i32, Stack)>,
    /// Blocks players used: a bench there opens once the step is over.
    uses: Vec<(i32, Pos)>,
}

/// A part's offset packed as the official server does (5 bits each:
/// x, z, y).
fn pack_filler(c: [i32; 3]) -> i32 {
    (c[0] & 31) | (c[2] & 31) << 5 | (c[1] & 31) << 10
}

fn unpack_filler(v: i32) -> [i32; 3] {
    let sign = |n: i32| if n & 16 != 0 { n | -32 } else { n };
    [sign(v & 31), sign(v >> 10 & 31), sign(v >> 5 & 31)]
}

fn json_ints(v: &[i32]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn sound_2d_packet(index: i32) -> Result<Packet> {
    packet(
        id::PLAY_SOUND_2D,
        &proto::PlaySoundEvent2D {
            sound_event_index: index,
            category: proto::SoundCategory::UI,
            volume_modifier: 1.0,
            pitch_modifier: 1.0,
        },
    )
}

fn sound_3d_packet(index: i32, category: proto::SoundCategory, at: [f64; 3]) -> Result<Packet> {
    packet(
        id::PLAY_SOUND_3D,
        &proto::PlaySoundEvent3D {
            sound_event_index: index,
            category,
            position: Some(proto::Position {
                x: at[0],
                y: at[1],
                z: at[2],
            }),
            volume_modifier: 1.0,
            pitch_modifier: 1.0,
        },
    )
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn section_of(x: i32, y: i32, z: i32) -> (Pos, i32) {
    (
        (x.div_euclid(32), y.div_euclid(32), z.div_euclid(32)),
        crate::chunk::index(
            x.rem_euclid(32) as usize,
            y.rem_euclid(32) as usize,
            z.rem_euclid(32) as usize,
        ) as i32,
    )
}

/// What connected blocks see of the world (spec 23).
struct ConnectView<'a> {
    g: &'a Inner,
    ground: &'a Ground,
    rules: &'a crate::support::Rules,
}
impl crate::connected::Cells for ConnectView<'_> {
    fn at(&self, p: Pos) -> Option<(i32, i32, i32)> {
        use crate::support::World;
        if !(0..crate::support::WORLD_HEIGHT).contains(&p.1) {
            return None;
        }
        let (id, rot) = View {
            g: self.g,
            ground: self.ground,
            hub_rules: self.rules,
        }
        .block(p.0, p.1, p.2);
        Some((id, rot, self.g.fillers.get(&p).copied().unwrap_or(0)))
    }
    fn rules(&self, block: i32) -> Option<&proto::ConnectedBlockRuleSet> {
        self.rules.block(block)?.connected.as_ref()
    }
    fn supports_up(&self, block: i32, rot: i32) -> bool {
        self.rules.supports_up(block, rot)
    }
    fn template_use(&self, block: i32) -> Option<&crate::connected::Use> {
        self.rules.template_use(block)
    }
    fn template(&self, name: &str) -> Option<&crate::connected::Template> {
        self.rules.template(name)
    }
    fn flip(&self, block: i32) -> u8 {
        self.rules.flip(block)
    }
    fn key(&self, block: i32) -> Option<&str> {
        self.rules.block(block).map(|b| b.key.as_str())
    }
    fn rule_set(&self, id: &str) -> Option<&proto::ConnectedBlockRuleSetAsset> {
        self.rules.rule_set(id)
    }
}

fn block_of(sec: Pos, index: i32) -> Pos {
    (
        sec.0 * 32 + (index & 31),
        sec.1 * 32 + (index >> 10),
        sec.2 * 32 + ((index >> 5) & 31),
    )
}

impl Hub {
    pub fn with_rules(mut self, r: crate::support::Rules) -> Self {
        self.rules = r;
        self
    }

    /// Whether a block counts for the height of its column (not
    /// transparent).
    pub fn opaque(&self, id: i32) -> bool {
        id != 0 && self.rules.block(id).is_none_or(|b| !b.transparent)
    }

    /// Marks a block for a support check at the next tick.
    fn mark(g: &mut Inner, x: i32, y: i32, z: i32) {
        if (0..crate::support::WORLD_HEIGHT).contains(&y) {
            let (sec, i) = section_of(x, y, z);
            g.ticking.insert((sec, i));
        }
    }

    /// Marks a block's hitbox grown by one block (3×3×3 for a plain block).
    fn mark_area(&self, g: &mut Inner, x: i32, y: i32, z: i32, id: i32, rot: i32) {
        let cells = self.rules.cells(id, rot);
        let lo = |a: usize| cells.iter().map(|c| c[a]).min().unwrap_or(0) - 1;
        let hi = |a: usize| cells.iter().map(|c| c[a]).max().unwrap_or(0) + 1;
        for dx in lo(0)..=hi(0) {
            for dy in lo(1)..=hi(1) {
                for dz in lo(2)..=hi(2) {
                    Self::mark(g, x + dx, y + dy, z + dz);
                }
            }
        }
    }

    /// The highest non-transparent block of a column at or below `from`.
    fn scan_top(&self, view: &View, x: i32, z: i32, from: i32) -> i32 {
        use crate::support::World;
        (0..=from)
            .rev()
            .find(|&y| self.opaque(view.block(x, y, z).0))
            .unwrap_or(i32::MIN)
    }

    fn top(&self, g: &mut Inner, ground: &Ground, x: i32, z: i32) -> i32 {
        if let Some(&t) = g.tops.get(&(x, z)) {
            return t;
        }
        let t = self.scan_top(
            &View {
                g,
                ground,
                hub_rules: &self.rules,
            },
            x,
            z,
            crate::support::WORLD_HEIGHT - 1,
        );
        g.tops.insert((x, z), t);
        t
    }

    /// Changes one block (spec 10): kept, queued for clients at the end of
    /// the tick, height, particles, support value, area marked when `how`
    /// says so. Returns the old block and rotation if it changed.
    #[allow(clippy::too_many_arguments)]
    fn change(
        &self,
        g: &mut Inner,
        ground: &Ground,
        (x, y, z): Pos,
        id: i32,
        rot: i32,
        how: How,
        out: &mut Outgoing,
    ) -> Option<(i32, i32)> {
        let old_filler = g.fillers.get(&(x, y, z)).copied().unwrap_or(0);
        let (old, old_rot) = self.set_cell(g, ground, (x, y, z), id, rot, 0, how, out)?;
        // A processing bench replaced by anything but one of its own
        // states drops what it held (spec 16).
        if g.processing.contains_key(&(x, y, z)) {
            let bench_of = |b: i32| {
                self.keys_by_id
                    .get(&b)
                    .and_then(|k| self.book.bench(k))
                    .filter(|b| b.processing.is_some())
                    .map(|b| b.id.clone())
            };
            if bench_of(id).is_none() || bench_of(id) != bench_of(old) {
                self.drop_processing(g, (x, y, z), out);
            }
        }
        // A container replaced by anything but one of its own states
        // drops what it held (spec 21).
        if g.containers.contains_key(&(x, y, z)) {
            let base = |b: i32| self.rules.container(b).map(|(_, base)| base.key.clone());
            if base(id).is_none() || base(id) != base(old) {
                self.drop_container(g, (x, y, z), out);
            }
        }
        // The old block's other parts go (spec 10, fillers).
        if old != 0 {
            self.remove_parts(g, ground, (x, y, z), old, old_rot, old_filler, how, out);
        }
        // The new block's other parts.
        if id != 0 {
            for c in self.rules.cells(id, rot) {
                if c == [0, 0, 0] {
                    continue;
                }
                let at = (x + c[0], y + c[1], z + c[2]);
                let prev_filler = g.fillers.get(&at).copied().unwrap_or(0);
                let filler = pack_filler(c);
                if let Some((prev, prev_rot)) =
                    self.set_cell(g, ground, at, id, rot, filler, how, out)
                    && prev != 0
                {
                    self.remove_parts(g, ground, at, prev, prev_rot, prev_filler, how, out);
                }
            }
        }
        Some((old, old_rot))
    }

    /// Removes the other parts of a multi-block block whose part at `at`
    /// had offset `filler`: the cells of its hitbox still holding it.
    #[allow(clippy::too_many_arguments)]
    fn remove_parts(
        &self,
        g: &mut Inner,
        ground: &Ground,
        at: Pos,
        block: i32,
        rot: i32,
        filler: i32,
        how: How,
        out: &mut Outgoing,
    ) {
        use crate::support::World;
        let f = unpack_filler(filler);
        let base = (at.0 - f[0], at.1 - f[1], at.2 - f[2]);
        for c in self.rules.cells(block, rot) {
            if c == f {
                continue;
            }
            let p = (base.0 + c[0], base.1 + c[1], base.2 + c[2]);
            let here = View {
                g,
                ground,
                hub_rules: &self.rules,
            }
            .block(p.0, p.1, p.2)
            .0;
            if here == block {
                self.set_cell(g, ground, p, 0, 0, 0, how, out);
            }
        }
    }

    /// Sets one cell (block, rotation, filler): kept, queued for clients,
    /// height, particles, support value, area marks. The old block and
    /// rotation when it changed.
    #[allow(clippy::too_many_arguments)]
    fn set_cell(
        &self,
        g: &mut Inner,
        ground: &Ground,
        (x, y, z): Pos,
        id: i32,
        rot: i32,
        filler: i32,
        how: How,
        out: &mut Outgoing,
    ) -> Option<(i32, i32)> {
        use crate::support::World;
        if !(0..crate::support::WORLD_HEIGHT).contains(&y) {
            return None;
        }
        let (old, old_rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(x, y, z);
        let old_filler = g.fillers.get(&(x, y, z)).copied().unwrap_or(0);
        if old == id && old_rot == rot && old_filler == filler {
            return None;
        }
        let old_top = self.top(g, ground, x, z);
        g.placed.insert((x, y, z), id);
        if rot != 0 {
            g.rotations.insert((x, y, z), rot);
        } else {
            g.rotations.remove(&(x, y, z));
        }
        if filler != 0 {
            g.fillers.insert((x, y, z), filler);
        } else {
            g.fillers.remove(&(x, y, z));
        }
        let (sec, index) = section_of(x, y, z);
        let tick = g.tick;
        g.sections.entry(sec).or_default().add(tick, index);
        g.touched.insert(sec);
        let new_top = if self.opaque(id) && y > old_top {
            y
        } else if y == old_top && !self.opaque(id) {
            self.scan_top(
                &View {
                    g,
                    ground,
                    hub_rules: &self.rules,
                },
                x,
                z,
                y - 1,
            )
        } else {
            old_top
        };
        if new_top != old_top {
            g.tops.insert((x, z), new_top);
            g.heights
                .entry((x.div_euclid(32), z.div_euclid(32)))
                .or_default()
                .insert((z.rem_euclid(32) << 5) | x.rem_euclid(32));
        }
        let at = Some(proto::Position {
            x: x as f64 + 0.5,
            y: y as f64 + 0.5,
            z: z as f64 + 0.5,
        });
        let particle = |block_id: i32, ev: proto::BlockParticleEvent| {
            packet(
                id::SPAWN_BLOCK_PARTICLE_SYSTEM,
                &proto::SpawnBlockParticleSystem {
                    block_id,
                    particle_type: ev,
                    position: at.clone(),
                },
            )
        };
        let gone = if how.by_physics {
            proto::BlockParticleEvent::PHYSICS
        } else {
            proto::BlockParticleEvent::BREAK
        };
        let parts = if old != 0 && id == 0 {
            vec![particle(old, gone)]
        } else if old == 0 && id != 0 {
            vec![particle(id, proto::BlockParticleEvent::BUILD)]
        } else {
            vec![
                particle(old, proto::BlockParticleEvent::BREAK),
                particle(id, proto::BlockParticleEvent::BUILD),
            ]
        };
        if !how.quiet {
            out.all.extend(parts.into_iter().flatten());
        }
        g.support.remove(&(x, y, z));
        if how.physics && filler == 0 {
            self.mark_area(g, x, y, z, id, rot);
        }
        self.save_change(x, y, z, id, rot, filler);
        Some((old, old_rot))
    }

    /// Runs `f` on the world with both locks, then sends what it produced.
    fn world_step<T>(&self, f: impl FnOnce(&Self, &mut Inner, &Ground, &mut Outgoing) -> T) -> T {
        let mut out = Outgoing::default();
        let r = {
            let ground = self.ground.lock().unwrap();
            let mut g = self.inner.lock().unwrap();
            f(self, &mut g, &ground, &mut out)
        };
        let mut placed = out.placed.into_iter().peekable();
        for (i, p) in out.all.iter().enumerate() {
            while let Some((_, to, q)) = placed.next_if(|(at, _, _)| *at <= i) {
                self.send(to, q);
            }
            self.broadcast(p);
        }
        for (_, to, q) in placed {
            self.send(to, q);
        }
        for (to, p) in out.to {
            self.send(to, p);
        }
        for (stacks, at) in out.drops {
            let stacks = stacks
                .into_iter()
                .map(|(item, n)| self.book.stack(&item, n))
                .collect();
            self.drop_items(stacks, at);
        }
        for (pid, st) in out.throws {
            self.throw_item(pid, st, 1.0);
        }
        for (pid, (x, y, z)) in out.uses {
            let _ = self.open_bench(pid, x, y, z);
        }
        r
    }

    /// The official `/block setticking`: one block checked next tick.
    pub fn set_ticking(&self, x: i32, y: i32, z: i32) {
        Self::mark(&mut self.inner.lock().unwrap(), x, y, z);
    }

    /// One world tick (30 a second): support checks of the blocks marked
    /// before it, then the block and height changes of the tick go out.
    pub fn tick(&self) {
        let now = std::time::Instant::now();
        let dt = match self.last_tick.lock().unwrap().replace(now) {
            Some(t) => (now - t).as_secs_f32(),
            None => crate::items::DT,
        };
        self.world_step(|hub, g, ground, out| {
            hub.send_player_updates(g, out);
            g.in_tick = true;
            // Effects tick before the chains that may add new ones; their
            // updates go out after.
            hub.effects_step(g, dt);
            hub.chains_step(g, ground, dt, out);
            hub.effects_updates(g);
            let todo = std::mem::take(&mut g.ticking);
            for (sec, index) in todo {
                hub.check_support(g, ground, block_of(sec, index), out);
            }
            // Before the flush: a bench's new block state goes out with
            // this tick's block changes, ahead of its window.
            hub.processing_step(g, ground, dt, out);
            hub.falling_step(g, ground, dt, out);
            hub.flush(g, ground, out);
            hub.health_step(g, dt, out);
            hub.stats_step(g, dt);
            hub.items_step(g, ground, dt, out);
            hub.crafting_step(g, ground, dt, out);
            hub.send_player_updates(g, out);
            g.in_tick = false;
            g.tick += 1;
            if g.tick % 150 == 0 {
                hub.save_states(g);
            }
        });
    }

    /// Every player's effects run one tick (spec 19): their stat changes,
    /// the ones that end, and the updates for viewers.
    fn effects_step(&self, g: &mut Inner, dt: f32) {
        use crate::stats::Predictable::All;
        let types = &self.stat_types;
        for e in g.players.values_mut() {
            if !e.effects.is_empty() {
                let who = StatWho {
                    creative: e.creative,
                    sprinting: e.sprinting,
                    gliding: e.gliding,
                };
                let Entry { effects, stats, .. } = e;
                let mut changes = Vec::new();
                let left = effects.tick(
                    &self.effects,
                    dt,
                    |x| stats.met(types, &who, &x.conditions),
                    |list, percent| changes.push((list.to_vec(), percent)),
                );
                for (list, percent) in changes {
                    for (i, amount) in list {
                        let Ok(i) = usize::try_from(i) else { continue };
                        let amount = match (percent, stats.max(i), stats.min(i)) {
                            (true, Some(max), Some(min)) => amount * (max - min) / 100.0,
                            _ => amount,
                        };
                        stats.add(types, i, amount, All);
                    }
                }
                if left {
                    e.mods_dirty = true;
                }
            }
        }
    }

    /// The effect changes of the tick, for every viewer.
    fn effects_updates(&self, g: &mut Inner) {
        let mut updates = Vec::new();
        for (pid, e) in g.players.iter_mut() {
            if let Some(u) = e.effects.take_changes() {
                updates.push((
                    *pid,
                    Audience::All,
                    proto::ComponentUpdate::EntityEffectsUpdate(Box::new(u)),
                ));
            }
        }
        g.stat_updates.extend(updates);
    }

    /// Every player's interaction chains run one tick (spec 18); the
    /// states go back to the player in one packet.
    fn chains_step(&self, g: &mut Inner, ground: &Ground, dt: f32, out: &mut Outgoing) {
        let mut ids: Vec<i32> = g
            .players
            .iter()
            .filter(|(_, e)| !e.chains.is_idle())
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        for pid in ids {
            let Some(mut manager) = g
                .players
                .get_mut(&pid)
                .map(|e| std::mem::take(&mut e.chains))
            else {
                continue;
            };
            manager.tick(
                &self.chains,
                &mut ChainHost {
                    hub: self,
                    g,
                    ground,
                    out,
                    pid,
                },
                dt,
            );
            if let Some(m) = manager.take_packet()
                && let Ok(p) = packet(id::SYNC_INTERACTION_CHAINS, &m)
            {
                out.to.push((pid, p));
            }
            if let Some(e) = g.players.get_mut(&pid) {
                e.chains = manager;
            }
        }
    }

    fn check_support(&self, g: &mut Inner, ground: &Ground, (x, y, z): Pos, out: &mut Outgoing) {
        use crate::support::{DECO, Found, World};
        let (id, rot) = View {
            g,
            ground,
            hub_rules: &self.rules,
        }
        .block(x, y, z);
        if id == 0 || g.support.get(&(x, y, z)) == Some(&DECO) || g.fillers.contains_key(&(x, y, z))
        {
            return;
        }
        let found = self.rules.check(
            &View {
                g,
                ground,
                hub_rules: &self.rules,
            },
            x,
            y,
            z,
        );
        let current = g.support.get(&(x, y, z)).copied().unwrap_or(0) as i32;
        match found {
            Found::NoRule => {}
            Found::Direct => {
                if current != 0 {
                    g.support.remove(&(x, y, z));
                    self.mark_neighbours(g, x, y, z, None);
                }
            }
            Found::Distance(d) => {
                if current == d {
                    self.mark_neighbours(g, x, y, z, Some(d - 1));
                } else {
                    g.support.insert((x, y, z), d as u8);
                    self.mark_neighbours(g, x, y, z, None);
                }
            }
            Found::Unsupported => self.unsupported(g, ground, (x, y, z), id, rot, out),
        }
    }

    /// Marks the 3×3×3 around a block, only those with a support value at
    /// most `max` when given.
    fn mark_neighbours(&self, g: &mut Inner, x: i32, y: i32, z: i32, max: Option<i32>) {
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -1..=1 {
                    let p = (x + dx, y + dy, z + dz);
                    let v = g.support.get(&p).copied().unwrap_or(0) as i32;
                    if max.is_none_or(|m| v <= m) {
                        Self::mark(g, p.0, p.1, p.2);
                    }
                }
            }
        }
    }

    /// A block without support (spec 10, "Removing an unsupported block").
    fn unsupported(
        &self,
        g: &mut Inner,
        ground: &Ground,
        (x, y, z): Pos,
        id: i32,
        rot: i32,
        out: &mut Outgoing,
    ) {
        let Some(b) = self.rules.block(id).cloned() else {
            return;
        };
        if b.drop_type == 2 {
            // It becomes an entity that falls (spec 24).
            let how = How {
                physics: true,
                ..Default::default()
            };
            if self.change(g, ground, (x, y, z), 0, 0, how, out).is_none() {
                return;
            }
            self.mark_area(g, x, y, z, id, rot);
            let collision = b.falling.map_or(-1, |f| f.collision);
            let bounds = self.rules.bounds_exact(id, rot);
            g.next_item = g.next_item.max(FIRST_ITEM_ID) + 1;
            let nid = g.next_item;
            g.falling.insert(
                nid,
                crate::falling::Falling::new(id, rot, (x, y, z), bounds, collision),
            );
            return;
        }
        if b.break_sound > 0 {
            let (cx, cy, cz) = (x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5);
            let reach = self.rules.sound_distance(b.break_sound) as f64;
            if let Ok(p) = packet(
                id::PLAY_SOUND_3D,
                &proto::PlaySoundEvent3D {
                    sound_event_index: b.break_sound,
                    category: proto::SoundCategory::SFX,
                    position: Some(proto::Position {
                        x: cx,
                        y: cy,
                        z: cz,
                    }),
                    volume_modifier: 1.0,
                    pitch_modifier: 1.0,
                },
            ) {
                for (&pid, e) in &g.players {
                    if let Some(at) = &e.position {
                        let d2 = (at.x - cx).powi(2) + (at.y - cy).powi(2) + (at.z - cz).powi(2);
                        if d2 <= reach * reach {
                            out.to.push((pid, p.clone()));
                        }
                    }
                }
            }
        }
        let how = How {
            physics: true,
            by_physics: true,
            ..Default::default()
        };
        self.change(g, ground, (x, y, z), 0, 0, how, out);
        self.mark_area(g, x, y, z, id, rot);
        if b.drop_type == 0 {
            let stacks = self.rules.roll(&b.physics_drops());
            out.drops
                .push((stacks, [x as f64 + 0.5, y as f64, z as f64 + 0.5]));
        }
    }

    /// Falling blocks (spec 24): each moves, and one that reached the
    /// ground acts on where it landed and is gone.
    fn falling_step(&self, g: &mut Inner, ground: &Ground, dt: f32, out: &mut Outgoing) {
        let ids: Vec<i32> = g.falling.keys().copied().collect();
        for nid in ids {
            let Some(mut f) = g.falling.get(&nid).cloned() else {
                continue;
            };
            let step = f.step(
                dt,
                &View {
                    g,
                    ground,
                    hub_rules: &self.rules,
                },
            );
            if f.pos[1] < -32.0 {
                g.falling.remove(&nid);
                if f.announced {
                    g.gone.push(nid);
                }
                continue;
            }
            if !step.landed {
                if step.moved {
                    g.falling_moved.push(nid);
                }
                g.falling.insert(nid, f);
                continue;
            }
            g.falling.remove(&nid);
            if f.announced {
                g.gone.push(nid);
            }
            self.falling_impact(g, ground, &f, out);
        }
    }

    /// What a landed falling block does (spec 24, "Landing").
    fn falling_impact(
        &self,
        g: &mut Inner,
        ground: &Ground,
        f: &crate::falling::Falling,
        out: &mut Outgoing,
    ) {
        use crate::support::World;
        let Some(b) = self.rules.block(f.block).cloned() else {
            return;
        };
        let cell = (
            f.pos[0].floor() as i32,
            f.pos[1].floor() as i32,
            f.pos[2].floor() as i32,
        );
        let settings = b.falling.unwrap_or(crate::support::Falling {
            impact: 0,
            drop_items: true,
            collision: -1,
        });
        match settings.impact {
            0 => {
                // Back into the world if every cell it needs is free.
                let free = self.rules.cells(f.block, f.rot).iter().all(|c| {
                    let (other, _) = View {
                        g,
                        ground,
                        hub_rules: &self.rules,
                    }
                    .block(cell.0 + c[0], cell.1 + c[1], cell.2 + c[2]);
                    other == 0 || self.rules.block(other).is_some_and(|o| !o.solid)
                });
                if free {
                    let how = How {
                        physics: true,
                        ..Default::default()
                    };
                    self.change(g, ground, cell, f.block, f.rot, how, out);
                    return;
                }
                self.falling_break(g, &b, f, true, out);
            }
            1 => self.falling_break(g, &b, f, settings.drop_items, out),
            _ => {
                // Explosions are not done yet: the block is just gone.
                warn!(
                    "{} landed at {} {} {} and would explode (explosions not done)",
                    b.key, cell.0, cell.1, cell.2
                );
            }
        }
    }

    /// A falling block breaking where it landed: particles for each of
    /// its cells, its break sound, and what it drops.
    fn falling_break(
        &self,
        g: &mut Inner,
        b: &crate::support::Block,
        f: &crate::falling::Falling,
        drops: bool,
        out: &mut Outgoing,
    ) {
        for c in self.rules.cells(f.block, f.rot) {
            if let Ok(p) = packet(
                id::SPAWN_BLOCK_PARTICLE_SYSTEM,
                &proto::SpawnBlockParticleSystem {
                    block_id: f.block,
                    particle_type: proto::BlockParticleEvent::BREAK,
                    position: Some(proto::Position {
                        x: f.pos[0] + c[0] as f64,
                        y: f.pos[1] + c[1] as f64,
                        z: f.pos[2] + c[2] as f64,
                    }),
                },
            ) {
                out.all.push(p);
            }
        }
        // The sound keeps its place after the particles.
        if b.break_sound > 0
            && let Ok(p) = sound_3d_packet(b.break_sound, proto::SoundCategory::SFX, f.pos)
        {
            let reach = self.rules.sound_distance(b.break_sound) as f64;
            for (&pid, e) in &g.players {
                if let Some(q) = &e.position
                    && dist2([q.x, q.y, q.z], f.pos) <= reach * reach
                {
                    out.placed.push((out.all.len(), pid, p.clone()));
                }
            }
        }
        if !drops {
            return;
        }
        // The soft drop when it has one, else the breaking drop.
        let drop = b
            .soft_drop
            .clone()
            .or_else(|| b.breaking_drop.clone())
            .map(|d| crate::support::Drop { quantity: 1, ..d });
        if let Some(d) = drop {
            let d = if d.item.is_none() && d.list.is_none() {
                crate::support::Drop {
                    item: b.item.clone(),
                    ..d
                }
            } else {
                d
            };
            let stacks = self.rules.roll(&d);
            if !stacks.is_empty() {
                out.drops
                    .push((stacks, [f.pos[0] + 0.5, f.pos[1], f.pos[2] + 0.5]));
            }
        }
    }

    /// The end of a tick: block changes per section, then heights.
    fn flush(&self, g: &mut Inner, ground: &Ground, out: &mut Outgoing) {
        use crate::support::World;
        let tick = g.tick;
        let touched = std::mem::take(&mut g.touched);
        for sec in touched {
            let list = g
                .sections
                .get_mut(&sec)
                .map(|s| s.take(tick))
                .unwrap_or_default();
            if list.is_empty() {
                continue;
            }
            let view = View {
                g,
                ground,
                hub_rules: &self.rules,
            };
            let p = if list.len() == 1 {
                let (x, y, z) = block_of(sec, list[0]);
                let (b, r) = view.block(x, y, z);
                let f = g.fillers.get(&(x, y, z)).copied().unwrap_or(0);
                packet(
                    id::SERVER_SET_BLOCK,
                    &proto::ServerSetBlock {
                        x,
                        y,
                        z,
                        block_id: b,
                        filler: f as i16,
                        rotation: r as i8,
                    },
                )
            } else if list.len() < 1024 {
                let cmds = list
                    .iter()
                    .map(|&i| {
                        let (x, y, z) = block_of(sec, i);
                        let (b, r) = view.block(x, y, z);
                        proto::SetBlockCmd {
                            index: i as i16,
                            block_id: b,
                            filler: g.fillers.get(&(x, y, z)).copied().unwrap_or(0) as i16,
                            rotation: r as i8,
                        }
                    })
                    .collect();
                packet(
                    id::SERVER_SET_BLOCKS,
                    &proto::ServerSetBlocks {
                        x: sec.0,
                        y: sec.1,
                        z: sec.2,
                        cmds,
                    },
                )
            } else {
                self.section_packet(&view, sec)
            };
            match p {
                Ok(p) => out.all.push(p),
                Err(e) => warn!("block changes not sent: {e:#}"),
            }
        }
        out.all.append(&mut g.deferred_particles);
        let heights = std::mem::take(&mut g.heights);
        for ((cx, cz), idx) in heights {
            let p = if idx.len() == 1 {
                let i = *idx.iter().next().unwrap();
                let (x, z) = (cx * 32 + (i & 31), cz * 32 + (i >> 5));
                let height = g.tops.get(&(x, z)).copied().unwrap_or(i32::MIN);
                packet(
                    id::SERVER_SET_HEIGHT,
                    &proto::ServerSetHeight { x, z, height },
                )
            } else if idx.len() < 512 {
                let heights = idx
                    .iter()
                    .map(|&i| proto::SetHeightCmd {
                        index: i as i16,
                        height: g
                            .tops
                            .get(&(cx * 32 + (i & 31), cz * 32 + (i >> 5)))
                            .copied()
                            .unwrap_or(i32::MIN),
                    })
                    .collect();
                packet(
                    id::SERVER_SET_HEIGHTS,
                    &proto::ServerSetHeights {
                        x: cx,
                        z: cz,
                        heights,
                    },
                )
            } else {
                let mut heightmap = Vec::with_capacity(1024);
                for z in 0..32 {
                    for x in 0..32 {
                        heightmap.push(self.top(g, ground, cx * 32 + x, cz * 32 + z));
                    }
                }
                packet(
                    id::SET_CHUNK_HEIGHTMAP,
                    &proto::SetChunkHeightmap {
                        x: cx,
                        z: cz,
                        heightmap,
                    },
                )
            };
            match p {
                Ok(p) => out.all.push(p),
                Err(e) => warn!("heights not sent: {e:#}"),
            }
        }
    }

    /// Dropped items for one tick (spec 11), then what clients see:
    /// `NetworkTick` and `EntityUpdates`.
    fn items_step(&self, g: &mut Inner, ground: &Ground, dt: f32, out: &mut Outgoing) {
        use crate::items;
        let tick = g.tick;
        let mut updates: Vec<proto::EntityUpdate> = Vec::new();
        let mut moved: Vec<i32> = Vec::new();
        // Physics.
        let ids: Vec<i32> = g.items.keys().copied().collect();
        for nid in &ids {
            let Some(mut it) = g.items.get(nid).cloned() else {
                continue;
            };
            if it.born > tick {
                continue;
            }
            let view = View {
                g,
                ground,
                hub_rules: &self.rules,
            };
            items::move_out(&mut it.pos, &mut it.vel, &view);
            items::gravity(&mut it.vel, dt);
            if items::movement(&mut it.pos, &mut it.vel, dt, &view) {
                moved.push(*nid);
            }
            if it.pos[1] < -32.0 {
                g.items.remove(nid);
                g.gone.push(*nid);
                continue;
            }
            g.items.insert(*nid, it);
        }
        // Merging.
        for nid in &ids {
            let Some(it) = g.items.get(nid).cloned() else {
                continue;
            };
            if it.born > tick {
                continue;
            }
            let max = self.book.max_stack(&it.item);
            if max <= 1 || it.quantity >= max {
                continue;
            }
            let poll = g
                .items
                .get_mut(nid)
                .map(|i| i.poll_merge_delay(dt))
                .unwrap_or(false);
            if !poll {
                continue;
            }
            let mut near: Vec<(f64, i32)> = g
                .items
                .iter()
                .filter(|(o, d)| *o != nid && d.born <= tick && d.stack().stacks_with(&it.stack()))
                .map(|(o, d)| (dist2(d.pos, it.pos), *o))
                .filter(|(d2, _)| *d2 <= items::MERGE_RADIUS * items::MERGE_RADIUS)
                .collect();
            // Nearest first; at the same distance the newest first (as the
            // official server's removal list shows).
            near.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
            let mut have = it.quantity;
            for (_, o) in near {
                let Some(other) = g.items.get(&o).cloned() else {
                    continue;
                };
                if other.quantity >= max {
                    continue;
                }
                let total = have + other.quantity;
                if total <= max {
                    g.items.remove(&o);
                    g.gone.push(o);
                    have = total;
                } else if let Some(x) = g.items.get_mut(&o) {
                    x.quantity = total - max;
                    x.stack_changed = true;
                    x.life = self.item_life(&x.item);
                    have = max;
                }
                if let Some(x) = g.items.get_mut(nid) {
                    x.quantity = have;
                    x.stack_changed = true;
                    x.life = self.item_life(&x.item);
                }
                if have >= max {
                    break;
                }
            }
        }
        // Pickup.
        let ids: Vec<i32> = g.items.keys().copied().collect();
        for nid in ids {
            let Some(mut it) = g.items.get(&nid).cloned() else {
                continue;
            };
            if it.born > tick || !it.poll_pickup_delay(dt) || !it.poll_throttle(dt) {
                g.items.insert(nid, it);
                continue;
            }
            g.items.insert(nid, it.clone());
            let mut players: Vec<(f64, i32)> = g
                .players
                .iter()
                .filter_map(|(pid, e)| {
                    let p = e.position.as_ref()?;
                    let d2 = dist2([p.x, p.y, p.z], it.pos);
                    (d2 <= items::PICKUP_RADIUS * items::PICKUP_RADIUS).then_some((d2, *pid))
                })
                .collect();
            players.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, pid) in players {
                let Some(e) = g.players.get_mut(&pid) else {
                    continue;
                };
                let before = e.inv.clone();
                // Into the container the player's preferences pick (spec 14).
                let items = ItemsView {
                    book: &self.book,
                    rules: &self.rules,
                    bench: None,
                    processing: None,
                    container: false,
                };
                let order = e.prefs.order_for(inventory::Items::kind(&items, &it.item));
                let left = e.inv.add_to(&items, &order, &it.stack());
                if left == it.quantity {
                    continue;
                }
                let taken = it.quantity - left;
                let changed = e.inv.changed_since(&before);
                e.inv_dirty.extend(changed);
                self.notify_pickup(pid, &it.stack().with_n(taken), it.pos, out);
                let target = e.position.as_ref().map_or(it.pos, |p| [p.x, p.y, p.z]);
                g.next_item = g.next_item.max(FIRST_ITEM_ID) + 1;
                let fid = g.next_item;
                if left == 0 {
                    g.items.remove(&nid);
                    g.gone.push(nid);
                    g.flights.insert(
                        fid,
                        items::Flight {
                            stack: it.stack(),
                            start: it.pos,
                            pos: it.pos,
                            to_player: pid,
                            life: items::FLIGHT_TIME,
                            announced: false,
                            finished: false,
                        },
                    );
                    let _ = target;
                    break;
                }
                it.quantity = left;
                it.stack_changed = true;
                it.life = self.item_life(&it.item);
                g.items.insert(nid, it.clone());
                g.flights.insert(
                    fid,
                    items::Flight {
                        stack: it.stack().with_n(left),
                        start: it.pos,
                        pos: it.pos,
                        to_player: pid,
                        life: items::FLIGHT_TIME,
                        announced: false,
                        finished: false,
                    },
                );
            }
        }
        // Lifetimes.
        let ids: Vec<i32> = g.items.keys().copied().collect();
        for nid in ids {
            if let Some(it) = g.items.get_mut(&nid)
                && it.born <= tick
            {
                it.life -= dt;
                if it.life <= 0.0 {
                    g.items.remove(&nid);
                    g.gone.push(nid);
                }
            }
        }
        // Flights to the players.
        let fids: Vec<i32> = g.flights.keys().copied().collect();
        for fid in fids {
            let Some(mut f) = g.flights.remove(&fid) else {
                continue;
            };
            if f.finished {
                g.gone.push(fid);
                continue;
            }
            let Some(p) = g.players.get(&f.to_player).and_then(|e| e.position.clone()) else {
                g.gone.push(fid);
                continue;
            };
            let target = [p.x, p.y + self.eye_height / 5.0, p.z];
            if f.step(target, dt) {
                f.finished = true;
            }
            if f.announced {
                updates.push(proto::EntityUpdate {
                    network_id: fid,
                    removed: None,
                    updates: Some(vec![transform_update(f.pos)]),
                });
            } else {
                f.announced = true;
                updates.push(proto::EntityUpdate {
                    network_id: fid,
                    removed: None,
                    updates: Some(vec![
                        item_update(&f.stack),
                        proto::ComponentUpdate::IntangibleUpdate(Box::default()),
                        transform_update(f.pos),
                    ]),
                });
            }
            g.flights.insert(fid, f);
        }
        // What clients see of the falling blocks.
        let falling_moved = std::mem::take(&mut g.falling_moved);
        for (nid, f) in g.falling.iter_mut() {
            if !f.announced {
                f.announced = true;
                updates.push(falling_entity(*nid, f));
            } else if falling_moved.contains(nid) {
                updates.push(proto::EntityUpdate {
                    network_id: *nid,
                    removed: None,
                    updates: Some(vec![falling_transform(f)]),
                });
            }
        }
        // What clients see of the items.
        for (nid, it) in g.items.iter_mut() {
            if it.born > tick {
                continue;
            }
            if !it.announced {
                it.announced = true;
                it.stack_changed = false;
                updates.push(item_entity(*nid, &it.stack(), it.pos));
                continue;
            }
            let mut parts = Vec::new();
            if it.stack_changed {
                it.stack_changed = false;
                parts.push(item_update(&it.stack()));
            }
            if moved.contains(nid) {
                parts.push(transform_update(it.pos));
            }
            if !parts.is_empty() {
                updates.push(proto::EntityUpdate {
                    network_id: *nid,
                    removed: None,
                    updates: Some(parts),
                });
            }
        }
        let gone = std::mem::take(&mut g.gone);
        if let Ok(p) = packet(
            crate::join::settings_id::NETWORK_TICK,
            &proto::NetworkTick {
                tick_duration_micros: (dt * 1_000_000.0) as i32,
                server_tick: tick as i32,
            },
        ) {
            out.all.push(p);
        }
        let stat_updates = std::mem::take(&mut g.stat_updates);
        if stat_updates.is_empty() {
            if (!updates.is_empty() || !gone.is_empty())
                && let Ok(p) = packet(
                    id::ENTITY_UPDATES,
                    &proto::EntityUpdates {
                        removed: (!gone.is_empty()).then_some(gone),
                        updates: (!updates.is_empty()).then_some(updates),
                    },
                )
            {
                out.all.push(p);
            }
            return;
        }
        // Stats changed: each player gets its own stats in full and the
        // others' shared stats, with the tick's other entity updates.
        let viewers: Vec<i32> = g
            .players
            .iter()
            .filter(|(_, e)| e.view.is_some())
            .map(|(i, _)| *i)
            .collect();
        for viewer in viewers {
            let mut mine = updates.clone();
            for (pid, audience, u) in &stat_updates {
                let sees = match audience {
                    Audience::Own => *pid == viewer,
                    Audience::Others => *pid != viewer,
                    Audience::All => true,
                };
                if sees {
                    mine.push(proto::EntityUpdate {
                        network_id: *pid,
                        removed: None,
                        updates: Some(vec![u.clone()]),
                    });
                }
            }
            if (!mine.is_empty() || !gone.is_empty())
                && let Ok(p) = packet(
                    id::ENTITY_UPDATES,
                    &proto::EntityUpdates {
                        removed: (!gone.is_empty()).then_some(gone.clone()),
                        updates: (!mine.is_empty()).then_some(mine),
                    },
                )
            {
                out.to.push((viewer, p));
            }
        }
    }

    /// The pickup notification and sound (spec 11).
    fn notify_pickup(&self, pid: i32, st: &Stack, at: [f64; 3], out: &mut Outgoing) {
        let item = st.item.as_str();
        let name = proto::FormattedMessage {
            message_id: Some(self.book.item_name(item)),
            ..Default::default()
        };
        let msg = proto::FormattedMessage {
            message_id: Some("server.general.pickedUpItem".into()),
            message_params: Some(vec![("item".into(), name)]),
            ..Default::default()
        };
        if let Ok(p) = packet(
            id::NOTIFICATION,
            &proto::Notification {
                message: Some(msg),
                secondary_message: None,
                icon: None,
                item: Some(st.packet()),
                style: Default::default(),
                tag: None,
            },
        ) {
            out.to.push((pid, p));
        }
        let sound = self.book.sound("SFX_Player_Pickup_Item");
        // Without a place (an item given straight to the player) the
        // sound is not positioned.
        if at[0].is_nan() {
            if sound > 0
                && let Ok(p) = sound_2d_packet(sound)
            {
                out.to.push((pid, p));
            }
            return;
        }
        if sound > 0
            && let Ok(p) = packet(
                id::PLAY_SOUND_3D,
                &proto::PlaySoundEvent3D {
                    sound_event_index: sound,
                    category: proto::SoundCategory::UI,
                    position: Some(proto::Position {
                        x: at[0],
                        y: at[1],
                        z: at[2],
                    }),
                    volume_modifier: 1.0,
                    pitch_modifier: 1.0,
                },
            )
        {
            out.to.push((pid, p));
        }
    }

    /// A whole section again (`SetChunk`), for 1024 changes or more.
    fn section_packet(&self, view: &View, sec: Pos) -> Result<Packet> {
        use crate::support::World;
        let mut s = crate::chunk::Section::default();
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    let (b, r) = view.block(
                        sec.0 * 32 + x as i32,
                        sec.1 * 32 + y as i32,
                        sec.2 * 32 + z as i32,
                    );
                    s.blocks[crate::chunk::index(x, y, z)] = b;
                    s.rotations[crate::chunk::index(x, y, z)] = r;
                }
            }
        }
        packet(
            id::SET_CHUNK,
            &proto::SetChunk {
                x: sec.0,
                y: sec.1,
                z: sec.2,
                local_light: Some(vec![0]),
                global_light: None,
                data: s.encode().map(|d| d.into_iter().map(|b| b as i8).collect()),
            },
        )
    }
}
