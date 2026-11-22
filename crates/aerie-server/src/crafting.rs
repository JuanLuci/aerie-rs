//! Crafting (spec 09): pocket crafting and benches.
//!
//! Recipes and item materials come from the registries Aerie builds
//! (`UpdateRecipes`, `UpdateItems`); benches from the `benches` table
//! (aerie-assets). A recipe can be made where one of its bench
//! requirements names the open bench (type and id; `Fieldcraft` for the
//! pocket window) at tier 1 without augment tags. Inputs are taken from
//! backpack, storage then hotbar: by item id, by tag, or by resource type
//! (an item gives its resource quantity per item). Creative players craft
//! without inputs. Outputs go to the inventory; what does not fit drops.

use std::collections::HashMap;

use aerie_protocol::proto;
use aerie_protocol::wire::Wire;

use crate::inventory::Inventory;

pub const BENCHES: &str = "benches";
/// The pocket window's bench id.
pub const FIELDCRAFT: &str = "Fieldcraft";
/// Registry packet ids.
const UPDATE_ITEMS: u32 = 54;
const UPDATE_RECIPES: u32 = 60;
const UPDATE_ITEM_SOUND_SETS: u32 = 43;
const UPDATE_BLOCK_GROUPS: u32 = 78;
/// The official defaults (GameplayConfig CraftingConfig, Memories).
const MAX_CHEST_COUNT: i32 = 100;
const CHEST_HORIZONTAL_RADIUS: i32 = 14;
const CHEST_VERTICAL_RADIUS: i32 = 6;
const MEMORIES_LEVEL: i32 = 1;

#[derive(Clone, Debug, Default)]
pub struct Bench {
    pub kind: i32,
    pub id: String,
    /// Sound event indexes (0 for none).
    pub open_sound: i32,
    pub close_sound: i32,
    pub completed_sound: i32,
    pub failed_sound: i32,
    /// (id, icon, name) in order.
    pub categories: Vec<(String, String, String)>,
    /// Structural benches: header categories, block group cycling,
    /// inventory hints always shown.
    pub header_categories: Vec<String>,
    pub cycling: bool,
    pub always_hints: bool,
    /// Processing benches: their asset (spec 16).
    pub processing: Option<crate::processing::Config>,
}

/// `BenchType` numbers.
pub const PROCESSING: i32 = 1;

/// A JSON object whose keys keep their first position when set again (as
/// the official window data does).
#[derive(Clone, Debug, Default)]
pub struct Data(Vec<(String, String)>);

impl Data {
    /// Sets `key` to a JSON value (already encoded).
    pub fn set(&mut self, key: &str, value: String) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(x) => x.1 = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }

    pub fn text(&self) -> String {
        let parts: Vec<String> = self
            .0
            .iter()
            .map(|(k, v)| format!("{}:{v}", json(k)))
            .collect();
        format!("{{{}}}", parts.join(","))
    }
}

/// The kinds of crafting windows Aerie opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pocket,
    /// A `Crafting` bench (workbench and the like).
    Simple,
    /// A `StructuralCrafting` bench (builder's bench).
    Structural,
    /// A `Processing` bench (furnace, campfire; spec 16).
    Processing,
    /// A block that holds items (a chest; spec 21).
    Container,
}

/// Options a structural window can show.
pub const OPTION_SLOTS: usize = 64;

/// A crafting window a player has open.
#[derive(Clone, Debug)]
pub struct Window {
    pub id: i32,
    pub kind: Kind,
    pub bench: Bench,
    /// The bench block's key and position; none for the pocket window.
    pub block: Option<(String, (i32, i32, i32))>,
    pub data: Data,
    /// Data changed: an `UpdateWindow` goes out at the end of the tick.
    pub dirty: bool,
    /// The extra resources (chests near the bench) go with the next update.
    pub extra_dirty: bool,
    /// Structural windows: the input slot, the options (item, quantity,
    /// recipe) and the selected option.
    pub input: crate::inventory::Slot,
    pub options: Vec<(crate::inventory::Stack, String)>,
    pub selected: i32,
    /// Processing windows: the bench's fuel, input and output slots as
    /// last sent.
    pub slots: Vec<crate::inventory::Slot>,
    /// Progress last sent and when (updates are rate limited).
    pub last_progress: f32,
    pub last_progress_at: Option<std::time::Instant>,
}

impl Window {
    /// The window of a block that holds items (spec 21).
    pub fn container(
        id: i32,
        block: &str,
        at: (i32, i32, i32),
        item: &str,
        slots: Vec<crate::inventory::Slot>,
    ) -> Self {
        let mut data = Data::default();
        data.set("blockItemId", json(item));
        Window {
            id,
            kind: Kind::Container,
            bench: Bench::default(),
            block: Some((block.to_owned(), at)),
            data,
            dirty: false,
            extra_dirty: false,
            input: None,
            options: Vec::new(),
            selected: 0,
            slots,
            last_progress: 0.0,
            last_progress_at: None,
        }
    }

    pub fn fieldcraft(book: &Book) -> Self {
        let mut data = Data::default();
        data.set("type", proto::BenchType::CRAFTING.0.to_string());
        data.set("id", json(FIELDCRAFT));
        data.set("name", json("client.inventory.fieldcraft.title"));
        data.set("categories", "[]".into());
        data.set("worldMemoriesLevel", MEMORIES_LEVEL.to_string());
        let _ = book;
        Window {
            id: 0,
            kind: Kind::Pocket,
            bench: Bench {
                kind: proto::BenchType::CRAFTING.0,
                id: FIELDCRAFT.into(),
                ..Default::default()
            },
            block: None,
            data,
            dirty: false,
            extra_dirty: false,
            input: None,
            options: Vec::new(),
            selected: 0,
            slots: Vec::new(),
            last_progress: 0.0,
            last_progress_at: None,
        }
    }

    /// New progress of the running craft; marks the window for an update
    /// as the official server does (a change of 5 %, half a second, done,
    /// going back, or the first time).
    pub fn progress(&mut self, p: f32) {
        self.data.set("progress", fmt_f32(p));
        if self.last_progress != p {
            let now = std::time::Instant::now();
            let long = self
                .last_progress_at
                .is_none_or(|t| now.duration_since(t).as_millis() > 500);
            if p >= 1.0 || p < self.last_progress || p - self.last_progress > 0.05 || long {
                self.last_progress = p;
                self.last_progress_at = Some(now);
                self.dirty = true;
            }
        }
    }

    /// The window's own slots as `InventorySection` (structural: the input
    /// then the options).
    pub fn section(&self) -> Option<proto::InventorySection> {
        if self.kind == Kind::Processing || self.kind == Kind::Container {
            return Some(proto::InventorySection {
                items: Some(crate::inventory::section_items(&self.slots)),
                capacity: self.slots.len() as i16,
            });
        }
        if self.kind != Kind::Structural {
            return None;
        }
        let mut slots = vec![self.input.clone()];
        slots.extend((0..OPTION_SLOTS).map(|i| self.options.get(i).map(|(s, _)| s.clone())));
        Some(proto::InventorySection {
            items: Some(crate::inventory::section_items(&slots)),
            capacity: (1 + OPTION_SLOTS) as i16,
        })
    }
}

/// A float as Java's `Float.toString` writes it for these values.
pub fn fmt_f32(v: f32) -> String {
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    }
}

/// One queued craft (timed recipes, structural benches).
#[derive(Clone, Debug)]
pub struct Job {
    pub recipe: String,
    pub quantity: i32,
    pub started: i32,
    pub completed: i32,
    pub seconds_done: f32,
    pub seconds: f32,
    /// Inputs come from the structural window's input slot.
    pub from_input: bool,
    /// What each started unit took, to give back if cancelled.
    pub taken: Vec<Vec<(String, i32)>>,
}

/// A player's crafting queue (at most 32 jobs).
#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub jobs: std::collections::VecDeque<Job>,
}

impl Queue {
    pub fn remaining(&self) -> i32 {
        self.jobs.iter().map(|j| j.quantity - j.completed).sum()
    }
}

#[derive(Clone, Debug, Default)]
struct ItemInfo {
    resources: Vec<(String, i32)>,
    tags: Vec<i32>,
    name: Option<String>,
    max_stack: i32,
    max_durability: f64,
    sound_set: i32,
    /// What the inventory moves look at (spec 14).
    armor_slot: Option<i32>,
    usable: bool,
    weapon_or_tool: bool,
    block_id: i32,
    ability: Option<crate::inventory::Ability>,
    /// `InteractionConfig.UseDistance` (adventure, creative).
    use_distance: (Option<f32>, Option<f32>),
    /// Stat modifiers and stats to clear (spec 17): armor, weapon, utility.
    stat_mods: ItemStats,
}

/// What an item does to its holder's stats (spec 17).
#[derive(Clone, Debug, Default)]
pub struct ItemStats {
    pub armor: Vec<(i32, Vec<crate::stats::Modifier>)>,
    pub weapon: Option<Vec<(i32, Vec<crate::stats::Modifier>)>>,
    pub weapon_clear: Vec<i32>,
    pub utility: Option<Vec<(i32, Vec<crate::stats::Modifier>)>>,
    pub utility_clear: Vec<i32>,
    pub utility_compatible: bool,
}

fn stat_mods(
    v: &Option<Vec<(i32, Vec<proto::Modifier>)>>,
) -> Option<Vec<(i32, Vec<crate::stats::Modifier>)>> {
    v.as_ref().map(|v| {
        v.iter()
            .map(|(i, ms)| {
                (
                    *i,
                    ms.iter()
                        .map(|m| crate::stats::Modifier {
                            max: m.target == proto::ModifierTarget::MAX,
                            multiplicative: m.calculation_type
                                == proto::CalculationType::MULTIPLICATIVE,
                            amount: m.amount,
                        })
                        .collect(),
                )
            })
            .collect()
    })
}

#[derive(Default)]
pub struct Book {
    recipes: HashMap<String, proto::CraftingRecipe>,
    items: HashMap<String, ItemInfo>,
    /// Block key → its bench.
    benches: HashMap<String, Bench>,
    memories_per_level: Vec<i32>,
    /// Sound name → event index.
    sounds: HashMap<String, i32>,
    /// Item sound set index → its `Drop` sound event.
    drop_sounds: HashMap<i32, i32>,
    /// Block groups (block group cycling at structural benches).
    groups: Vec<Vec<String>>,
    /// Items whose `FuelQuality` is not 1.
    fuel_quality: HashMap<String, f64>,
}

/// Why a craft did not happen.
#[derive(Debug, PartialEq)]
pub enum Refused {
    /// Unknown recipe: the official server disconnects.
    Unknown,
    /// Not made at this bench (or needs knowledge, memories, augments).
    WrongBench,
}

fn dash(s: &str) -> String {
    if s == "-" {
        String::new()
    } else {
        s.to_owned()
    }
}

fn json(s: &str) -> String {
    let mut o = String::from('"');
    for c in s.chars() {
        match c {
            '"' => o += "\\\"",
            '\\' => o += "\\\\",
            c if (c as u32) < 0x20 => o += &format!("\\u{:04x}", c as u32),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

impl Book {
    pub fn new(registries: &[(u32, Vec<u8>)], benches: &str) -> Self {
        let mut book = Book {
            memories_per_level: vec![10, 25, 50, 100, 200],
            ..Default::default()
        };
        for (id, payload) in registries {
            match *id {
                UPDATE_RECIPES => {
                    if let Ok((r, _)) = proto::UpdateRecipes::decode(payload, 0) {
                        book.recipes.extend(r.recipes.into_iter().flatten());
                    }
                }
                UPDATE_ITEMS => {
                    if let Ok((r, _)) = proto::UpdateItems::decode(payload, 0) {
                        for (k, it) in r.items.into_iter().flatten() {
                            book.items.insert(
                                k,
                                ItemInfo {
                                    resources: it
                                        .resource_types
                                        .into_iter()
                                        .flatten()
                                        .filter_map(|t| Some((t.id?, t.quantity)))
                                        .collect(),
                                    tags: it.tag_indexes.unwrap_or_default(),
                                    name: it.translation_properties.and_then(|t| t.name),
                                    max_stack: it.max_stack,
                                    max_durability: it.durability,
                                    armor_slot: it.armor.as_ref().map(|a| a.armor_slot.0),
                                    usable: it.utility.as_ref().is_some_and(|u| u.usable),
                                    weapon_or_tool: it.weapon.is_some() || it.tool.is_some(),
                                    block_id: it.block_id,
                                    stat_mods: ItemStats {
                                        armor: it
                                            .armor
                                            .as_ref()
                                            .and_then(|a| stat_mods(&a.stat_modifiers))
                                            .unwrap_or_default(),
                                        weapon: it
                                            .weapon
                                            .as_ref()
                                            .and_then(|w| stat_mods(&w.stat_modifiers)),
                                        weapon_clear: it
                                            .weapon
                                            .as_ref()
                                            .and_then(|w| w.entity_stats_to_clear.clone())
                                            .unwrap_or_default(),
                                        utility: it
                                            .utility
                                            .as_ref()
                                            .and_then(|u| stat_mods(&u.stat_modifiers)),
                                        utility_clear: it
                                            .utility
                                            .as_ref()
                                            .and_then(|u| u.entity_stats_to_clear.clone())
                                            .unwrap_or_default(),
                                        utility_compatible: it
                                            .utility
                                            .as_ref()
                                            .is_some_and(|u| u.compatible),
                                    },
                                    use_distance: it
                                        .interaction_config
                                        .as_ref()
                                        .and_then(|c| c.use_distance.as_ref())
                                        .map_or((None, None), |m| {
                                            let get = |g: proto::GameMode| {
                                                m.iter().find(|(k, _)| *k == g).map(|(_, v)| *v)
                                            };
                                            (
                                                get(proto::GameMode::ADVENTURE).or(Some(1.0)),
                                                get(proto::GameMode::CREATIVE).or(Some(1.0)),
                                            )
                                        }),
                                    ability: it.ability.as_ref().map(|a| {
                                        crate::inventory::Ability {
                                            core: a.slot == proto::AbilitySlot::CORE,
                                            keywords: a
                                                .ability_keyword_indexes
                                                .clone()
                                                .unwrap_or_default(),
                                            requires_any: a
                                                .requires_any_ability_keyword_indexes
                                                .clone(),
                                            requires_all: a
                                                .requires_all_ability_keyword_indexes
                                                .clone(),
                                            excludes_any: a
                                                .excludes_any_ability_keyword_indexes
                                                .clone(),
                                        }
                                    }),
                                    sound_set: it.item_sound_set_index,
                                },
                            );
                        }
                    }
                }
                UPDATE_ITEM_SOUND_SETS => {
                    if let Ok((r, _)) = proto::UpdateItemSoundSets::decode(payload, 0) {
                        for (i, set) in r.item_sound_sets.into_iter().flatten() {
                            if let Some((_, e)) = set
                                .sound_event_indices
                                .iter()
                                .flatten()
                                .find(|(k, _)| *k == proto::ItemSoundEvent::DROP)
                            {
                                book.drop_sounds.insert(i, *e);
                            }
                        }
                    }
                }
                UPDATE_BLOCK_GROUPS => {
                    if let Ok((r, _)) = proto::UpdateBlockGroups::decode(payload, 0) {
                        for (_, g) in r.groups.into_iter().flatten() {
                            book.groups.push(g.names.unwrap_or_default());
                        }
                    }
                }
                _ => {}
            }
        }
        for l in benches.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w[..] {
                ["W", block, kind, id, open, close, completed, failed] => {
                    book.benches.insert(
                        block.to_owned(),
                        Bench {
                            kind: kind.parse().unwrap_or(0),
                            id: id.to_owned(),
                            open_sound: open.parse().unwrap_or(0),
                            close_sound: close.parse().unwrap_or(0),
                            completed_sound: completed.parse().unwrap_or(0),
                            failed_sound: failed.parse().unwrap_or(0),
                            ..Default::default()
                        },
                    );
                }
                ["S", name, index] => {
                    book.sounds
                        .insert(name.to_owned(), index.parse().unwrap_or(0));
                }
                ["C", block, id, icon, name] => {
                    if let Some(b) = book.benches.get_mut(block) {
                        b.categories.push((dash(id), dash(icon), dash(name)));
                    }
                }
                ["H", block, cat] => {
                    if let Some(b) = book.benches.get_mut(block) {
                        b.header_categories.push(cat.to_owned());
                    }
                }
                ["F", block, cycling, hints] => {
                    if let Some(b) = book.benches.get_mut(block) {
                        b.cycling = cycling == "1";
                        b.always_hints = hints == "1";
                    }
                }
                ["Q", item, q] => {
                    book.fuel_quality
                        .insert(item.to_owned(), q.parse().unwrap_or(1.0));
                }
                _ => {}
            }
        }
        for (block, b) in book.benches.iter_mut() {
            if b.kind == PROCESSING {
                b.processing = crate::processing::parse(benches, block);
            }
        }
        book
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.recipes.len(), self.items.len(), self.benches.len())
    }

    /// How far a player holding `item` may use blocks (the official
    /// defaults 5 in adventure, 6 in creative; an item's own map without
    /// the mode gives 1).
    pub fn use_distance(&self, item: Option<&str>, creative: bool) -> f32 {
        let d = item
            .and_then(|i| self.items.get(i))
            .map_or((None, None), |i| i.use_distance);
        if creative {
            d.1.unwrap_or(6.0)
        } else {
            d.0.unwrap_or(5.0)
        }
    }

    pub fn item_stats(&self, item: &str) -> Option<&ItemStats> {
        self.items.get(item).map(|i| &i.stat_mods)
    }

    pub fn armor_slot(&self, item: &str) -> Option<i32> {
        self.items.get(item)?.armor_slot
    }

    pub fn usable(&self, item: &str) -> bool {
        self.items.get(item).is_some_and(|i| i.usable)
    }

    pub fn weapon_or_tool(&self, item: &str) -> bool {
        self.items.get(item).is_some_and(|i| i.weapon_or_tool)
    }

    /// The block an item places (0 or less: none).
    pub fn block_of(&self, item: &str) -> i32 {
        self.items.get(item).map_or(0, |i| i.block_id)
    }

    pub fn ability(&self, item: &str) -> Option<crate::inventory::Ability> {
        self.items.get(item)?.ability.clone()
    }

    /// An item's `MaxDurability` (0: it never wears).
    pub fn max_durability(&self, item: &str) -> f64 {
        self.items.get(item).map_or(0.0, |i| i.max_durability)
    }

    /// A new stack of `item` at full durability, as the official server
    /// makes one from an item id.
    pub fn stack(&self, item: &str, n: i32) -> crate::inventory::Stack {
        crate::inventory::Stack::new(item, n, self.max_durability(item))
    }

    /// Most of an item in one slot (the official default 100 for an unknown
    /// item).
    pub fn max_stack(&self, item: &str) -> i32 {
        self.items
            .get(item)
            .map(|i| i.max_stack)
            .filter(|&m| m > 0)
            .unwrap_or(100)
    }

    /// The item's name key, for messages.
    pub fn item_name(&self, item: &str) -> String {
        self.items
            .get(item)
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| format!("server.items.{item}.name"))
    }

    /// A sound event index by name, 0 when unknown.
    pub fn sound(&self, name: &str) -> i32 {
        self.sounds.get(name).copied().unwrap_or(0)
    }

    pub fn bench(&self, block: &str) -> Option<&Bench> {
        self.benches.get(block)
    }

    /// Recipes made at bench `id` in category `cat`, sorted.
    fn recipes_for(&self, id: &str, cat: &str) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .recipes
            .iter()
            .filter(|(_, r)| {
                r.bench_requirement
                    .iter()
                    .flatten()
                    .any(|b| b.id == id && b.categories.iter().flatten().any(|c| c == cat))
            })
            .map(|(k, _)| k.as_str())
            .collect();
        v.sort();
        v
    }

    /// A bench window as the official server opens it (spec 09): its data
    /// in the official key order.
    pub fn bench_window(
        &self,
        id: i32,
        block: &str,
        at: (i32, i32, i32),
        bench: &Bench,
    ) -> Option<Window> {
        let kind = match proto::BenchType(bench.kind) {
            proto::BenchType::CRAFTING => Kind::Simple,
            proto::BenchType::STRUCTURAL_CRAFTING => Kind::Structural,
            _ => return None,
        };
        let name = self.item_name(block);
        let mut d = Data::default();
        d.set("type", bench.kind.to_string());
        d.set("id", json(&bench.id));
        d.set("name", json(&name));
        d.set("blockItemId", json(block));
        d.set("tierLevel", "1".into());
        d.set("grantedAugmentTags", "[]".into());
        match kind {
            Kind::Simple => {
                let cats: Vec<String> = bench
                    .categories
                    .iter()
                    .map(|(cid, icon, n)| {
                        let recipes: Vec<String> = self
                            .recipes_for(&bench.id, cid)
                            .iter()
                            .map(|r| json(r))
                            .collect();
                        format!(
                            "{{\"id\":{},\"name\":{},\"icon\":{},\"craftableRecipes\":[{}]}}",
                            json(cid),
                            json(n),
                            json(icon),
                            recipes.join(",")
                        )
                    })
                    .collect();
                d.set("categories", format!("[{}]", cats.join(",")));
            }
            Kind::Structural => {
                d.set("selected", "0".into());
                d.set("allowBlockGroupCycling", bench.cycling.to_string());
                d.set("alwaysShowInventoryHints", bench.always_hints.to_string());
            }
            Kind::Pocket | Kind::Processing | Kind::Container => {}
        }
        d.set("worldMemoriesLevel", MEMORIES_LEVEL.to_string());
        d.set("nearbyChestCount", "0".into());
        d.set("maxChestCount", MAX_CHEST_COUNT.to_string());
        d.set("chestHorizontalRadius", CHEST_HORIZONTAL_RADIUS.to_string());
        d.set("chestVerticalRadius", CHEST_VERTICAL_RADIUS.to_string());
        let per: Vec<String> = self
            .memories_per_level
            .iter()
            .map(|n| n.to_string())
            .collect();
        d.set("memoriesPerLevel", format!("[{}]", per.join(",")));
        Some(Window {
            id,
            kind,
            bench: bench.clone(),
            block: Some((block.to_owned(), at)),
            data: d,
            dirty: false,
            extra_dirty: false,
            input: None,
            options: Vec::new(),
            selected: 0,
            slots: Vec::new(),
            last_progress: 0.0,
            last_progress_at: None,
        })
    }

    /// A processing bench's window (spec 16) for its state, its data keys
    /// in the official order.
    pub fn processing_window(
        &self,
        id: i32,
        block: &str,
        at: (i32, i32, i32),
        bench: &Bench,
        state: &crate::processing::State,
        progress: f32,
    ) -> Option<Window> {
        let cfg = bench.processing.as_ref()?;
        let name = self.item_name(block);
        let mut d = Data::default();
        d.set("type", bench.kind.to_string());
        d.set("id", json(&bench.id));
        d.set("name", json(&name));
        d.set("blockItemId", json(block));
        d.set("tierLevel", "1".into());
        d.set("grantedAugmentTags", "[]".into());
        d.set("active", state.active.to_string());
        d.set("progress", fmt_f32(progress));
        let opt = |s: &Option<String>| s.as_ref().map_or("null".into(), |v| json(v));
        if let Some(fuel) = cfg.fuel.as_ref().filter(|f| !f.is_empty()) {
            let v: Vec<String> = fuel
                .iter()
                .map(|f| {
                    format!(
                        "{{\"icon\":{},\"resourceTypeId\":{}}}",
                        opt(&f.icon),
                        opt(&f.resource)
                    )
                })
                .collect();
            d.set("fuel", format!("[{}]", v.join(",")));
        }
        d.set("maxFuel", cfg.max_fuel.max(0).to_string());
        d.set("fuelTime", fmt_f32(0.0));
        d.set("progress", fmt_f32(progress));
        d.set("processingFuelSlots", "0".into());
        d.set("processingSlots", "0".into());
        let inputs = cfg.inputs(1);
        if !inputs.is_empty() {
            let v: Vec<String> = inputs
                .iter()
                .map(|i| format!("{{\"icon\":{}}}", opt(&i.icon)))
                .collect();
            d.set("input", format!("[{}]", v.join(",")));
        }
        d.set("outputSlotsCount", cfg.outputs(1).to_string());
        d.set("worldMemoriesLevel", MEMORIES_LEVEL.to_string());
        d.set("nearbyChestCount", "0".into());
        d.set("maxChestCount", MAX_CHEST_COUNT.to_string());
        d.set("chestHorizontalRadius", CHEST_HORIZONTAL_RADIUS.to_string());
        d.set("chestVerticalRadius", CHEST_VERTICAL_RADIUS.to_string());
        Some(Window {
            id,
            kind: Kind::Processing,
            bench: bench.clone(),
            block: Some((block.to_owned(), at)),
            data: d,
            dirty: false,
            extra_dirty: false,
            input: None,
            options: Vec::new(),
            selected: 0,
            slots: state.slots(),
            last_progress: 0.0,
            last_progress_at: None,
        })
    }

    /// Recipes made at a bench (type and id), by id.
    fn bench_recipes(&self, kind: i32, bench: &str) -> Vec<(&String, &proto::CraftingRecipe)> {
        let mut v: Vec<_> = self
            .recipes
            .iter()
            .filter(|(_, r)| {
                r.bench_requirement
                    .iter()
                    .flatten()
                    .any(|b| b.r#type.0 == kind && b.id == bench)
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }

    /// Whether `item` is the material `m`.
    fn matches(&self, m: &proto::MaterialQuantity, item: &str) -> bool {
        self.units(m, item).is_some()
    }

    /// Structural benches: the recipes an input stack can become, in the
    /// official order (header categories first, then by category, then id).
    pub fn structural_options(
        &self,
        bench: &Bench,
        input: &str,
    ) -> (Vec<(crate::inventory::Stack, String)>, usize) {
        let header = |r: &proto::CraftingRecipe| {
            r.bench_requirement.iter().flatten().any(|b| {
                b.r#type.0 == bench.kind
                    && b.id == bench.id
                    && b.categories
                        .iter()
                        .flatten()
                        .any(|c| bench.header_categories.contains(c))
            })
        };
        let priority = |r: &proto::CraftingRecipe| {
            r.bench_requirement
                .iter()
                .flatten()
                .find(|b| b.r#type.0 == bench.kind && b.id == bench.id && b.categories.is_some())
                .map(|b| {
                    b.categories
                        .iter()
                        .flatten()
                        .map(|c| {
                            bench
                                .categories
                                .iter()
                                .position(|(id, _, _)| id == c)
                                .map_or(i32::MAX, |p| p as i32)
                        })
                        .min()
                        .unwrap_or(i32::MAX)
                })
                .unwrap_or(i32::MAX)
        };
        let mut found: Vec<(&String, &proto::CraftingRecipe)> = self
            .bench_recipes(bench.kind, &bench.id)
            .into_iter()
            .filter(|(_, r)| {
                let ins = r.inputs.as_deref().unwrap_or(&[]);
                ins.len() == 1 && self.matches(&ins[0], input)
            })
            .collect();
        found.sort_by(|a, b| {
            header(b.1)
                .cmp(&header(a.1))
                .then(priority(a.1).cmp(&priority(b.1)))
                .then(a.0.cmp(b.0))
        });
        let divider = found.iter().take_while(|(_, r)| header(r)).count();
        let mut options = Vec::new();
        for (id, r) in found {
            for b in r.bench_requirement.iter().flatten() {
                if b.r#type.0 == bench.kind && b.id == bench.id && options.len() < OPTION_SLOTS {
                    if let Some(o) = r.outputs.iter().flatten().next()
                        && let Some(item) = &o.item_id
                    {
                        options.push((self.stack(item, o.quantity), id.clone()));
                    }
                }
            }
        }
        (options, divider)
    }

    /// Hotbar-then-storage slot indexes holding something a bench recipe
    /// takes as its first input (structural `inventoryHints`).
    pub fn inventory_hints(&self, bench: &Bench, inv: &Inventory) -> Vec<i32> {
        let recipes = self.bench_recipes(bench.kind, &bench.id);
        let mut out = Vec::new();
        let mut i = 0;
        for (sec, n) in [
            (crate::inventory::HOTBAR, 9),
            (crate::inventory::STORAGE, 36),
        ] {
            for slot in 0..n {
                if let Some(crate::inventory::Stack { item, .. }) = inv.get(sec, slot)
                    && recipes.iter().any(|(_, r)| {
                        r.inputs
                            .iter()
                            .flatten()
                            .next()
                            .is_some_and(|m| self.matches(m, &item))
                    })
                {
                    out.push(i);
                }
                i += 1;
            }
        }
        out
    }

    /// The block group an item is in, for cycling.
    pub fn group_of(&self, item: &str) -> Option<&Vec<String>> {
        self.groups.iter().find(|g| g.iter().any(|n| n == item))
    }

    /// The item's `Drop` sound (0 for none).
    pub fn drop_sound(&self, item: &str) -> i32 {
        self.items
            .get(item)
            .and_then(|i| self.drop_sounds.get(&i.sound_set))
            .copied()
            .unwrap_or(0)
    }

    pub fn recipe(&self, id: &str) -> Option<&proto::CraftingRecipe> {
        self.recipes.get(id)
    }

    /// Most crafts the inventory could hold of the recipe's primary
    /// output (the official ceiling on a requested quantity).
    pub fn ceiling(&self, id: &str) -> i32 {
        let Some(r) = self.recipes.get(id) else {
            return 1;
        };
        let Some(p) = r.primary_output.as_ref() else {
            return 1;
        };
        let Some(item) = &p.item_id else { return 1 };
        let per = p.quantity.max(1) as i64;
        let max = self.max_stack(item).max(1) as i64;
        (45 * max / per).clamp(1, i32::MAX as i64) as i32
    }

    /// Whether recipe `id` can be made at bench (`kind`, `bench`) (spec 09).
    pub fn allowed(&self, id: &str, kind: i32, bench: &str) -> Result<(), Refused> {
        let r = self.recipes.get(id).ok_or(Refused::Unknown)?;
        let at_bench = r.bench_requirement.iter().flatten().any(|b| {
            b.r#type.0 == kind
                && b.id == bench
                && b.required_tier_level <= 1
                && b.required_augment_tags
                    .as_ref()
                    .is_none_or(|t| t.is_empty())
        });
        if !at_bench || r.knowledge_required || r.required_memories_level > MEMORIES_LEVEL {
            return Err(Refused::WrongBench);
        }
        Ok(())
    }

    /// Takes one unit's inputs from the player's inventory; what it took.
    pub fn take_unit(&self, inv: &mut Inventory, id: &str) -> Option<Vec<(String, i32)>> {
        let r = self.recipes.get(id)?;
        let mut after = inv.clone();
        for m in r.inputs.iter().flatten() {
            if after.take_units(|item| self.units(m, item), m.quantity) > 0 {
                return None;
            }
        }
        let taken = diff(inv, &after);
        *inv = after;
        Some(taken)
    }

    /// The outputs of `quantity` crafts.
    pub fn outputs(&self, id: &str, quantity: i32) -> Vec<(String, i32)> {
        self.recipes
            .get(id)
            .map(|r| {
                r.outputs
                    .iter()
                    .flatten()
                    .filter_map(|m| Some((m.item_id.clone()?, m.quantity * quantity)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The recipe's time in seconds.
    pub fn seconds(&self, id: &str) -> f32 {
        self.recipes.get(id).map_or(0.0, |r| r.time_seconds)
    }

    /// The input material of a one-input recipe and its quantity.
    pub fn single_input(&self, id: &str) -> Option<&proto::MaterialQuantity> {
        self.recipes.get(id)?.inputs.as_ref()?.first()
    }

    /// A processing bench's recipes at a tier, as the processing code
    /// takes them (spec 16).
    pub fn processing_recipes(&self, bench: &Bench, tier: i32) -> Vec<crate::processing::Recipe> {
        self.bench_recipes(bench.kind, &bench.id)
            .into_iter()
            .filter(|(_, r)| {
                !r.bench_requirement
                    .iter()
                    .flatten()
                    .any(|b| b.id == bench.id && b.required_tier_level > tier)
            })
            .map(|(id, r)| crate::processing::Recipe {
                id: id.clone(),
                inputs: r
                    .inputs
                    .iter()
                    .flatten()
                    .map(|m| crate::processing::Material {
                        item: m.item_id.clone(),
                        resource: m.resource_type_id.clone(),
                        tag: (m.item_tag != i32::MIN).then_some(m.item_tag),
                        quantity: m.quantity,
                    })
                    .collect(),
                outputs: r
                    .outputs
                    .iter()
                    .flatten()
                    .filter_map(|m| Some((m.item_id.clone()?, m.quantity)))
                    .collect(),
                seconds: r.time_seconds,
            })
            .collect()
    }

    /// Units of a processing material one item gives.
    pub fn processing_units(&self, m: &crate::processing::Material, item: &str) -> Option<i32> {
        let q = proto::MaterialQuantity {
            item_id: m.item.clone(),
            item_tag: m.tag.unwrap_or(i32::MIN),
            resource_type_id: m.resource.clone(),
            quantity: m.quantity,
        };
        self.units(&q, item)
    }

    /// Units of a resource type one item gives.
    pub fn resource_units(&self, item: &str, resource: &str) -> Option<i32> {
        self.items
            .get(item)?
            .resources
            .iter()
            .find(|(id, _)| id == resource)
            .map(|(_, q)| *q)
    }

    pub fn fuel_quality(&self, item: &str) -> f64 {
        self.fuel_quality.get(item).copied().unwrap_or(1.0)
    }

    /// Whether `item` is the material.
    pub fn is_material(&self, m: &proto::MaterialQuantity, item: &str) -> bool {
        self.matches(m, item)
    }

    /// Units of material `m` one `item` gives, if it is that material.
    fn units(&self, m: &proto::MaterialQuantity, item: &str) -> Option<i32> {
        if let Some(id) = &m.item_id {
            return (id == item).then_some(1);
        }
        let info = self.items.get(item)?;
        if let Some(r) = &m.resource_type_id {
            return info
                .resources
                .iter()
                .find(|(id, _)| id == r)
                .map(|(_, q)| *q);
        }
        (m.item_tag != i32::MIN && info.tags.contains(&m.item_tag)).then_some(1)
    }
}

/// Items `before` had that `after` lacks (what a removal took).
fn diff(before: &Inventory, after: &Inventory) -> Vec<(String, i32)> {
    let mut count: std::collections::BTreeMap<String, i32> = Default::default();
    for (item, n) in before.all() {
        *count.entry(item).or_default() += n;
    }
    for (item, n) in after.all() {
        *count.entry(item).or_default() -= n;
    }
    count.into_iter().filter(|(_, n)| *n > 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Book {
        let mut b = Book::default();
        let mat = |item: Option<&str>, res: Option<&str>, q| proto::MaterialQuantity {
            item_id: item.map(Into::into),
            item_tag: i32::MIN,
            resource_type_id: res.map(Into::into),
            quantity: q,
        };
        b.recipes.insert(
            "Bench_Recipe".into(),
            proto::CraftingRecipe {
                id: Some("Bench_Recipe".into()),
                inputs: Some(vec![
                    mat(None, Some("Wood_Trunk"), 4),
                    mat(Some("Rock"), None, 3),
                ]),
                outputs: Some(vec![mat(Some("Bench"), None, 1)]),
                bench_requirement: Some(vec![proto::BenchRequirement {
                    r#type: proto::BenchType::CRAFTING,
                    id: FIELDCRAFT.into(),
                    categories: Some(vec!["Tools".into()]),
                    required_tier_level: 0,
                    required_augment_tags: None,
                }]),
                required_memories_level: 1,
                ..Default::default()
            },
        );
        b.items.insert(
            "Log".into(),
            ItemInfo {
                resources: vec![("Wood_Trunk".into(), 1)],
                ..Default::default()
            },
        );
        b
    }

    #[test]
    fn units_resources_and_items() {
        use crate::inventory::Stack;
        let b = book();
        let mut inv = Inventory::default();
        inv.add(&Stack::new("Log", 5, 0.0), 100);
        inv.add(&Stack::new("Rock", 2, 0.0), 100);
        assert_eq!(b.take_unit(&mut inv, "Bench_Recipe"), None);
        assert_eq!(
            inv.get(-1, 0),
            Some(Stack::new("Log", 5, 0.0)),
            "nothing taken"
        );
        inv.add(&Stack::new("Rock", 1, 0.0), 100);
        assert_eq!(
            b.take_unit(&mut inv, "Bench_Recipe"),
            Some(vec![("Log".into(), 4), ("Rock".into(), 3)])
        );
        assert_eq!(inv.get(-1, 0), Some(Stack::new("Log", 1, 0.0)));
        assert_eq!(
            b.allowed("Bench_Recipe", 0, "Workbench"),
            Err(Refused::WrongBench)
        );
        assert_eq!(b.allowed("Nope", 0, FIELDCRAFT), Err(Refused::Unknown));
        assert_eq!(b.outputs("Bench_Recipe", 2), vec![("Bench".into(), 2)]);
    }

    #[test]
    fn bench_window_lists_recipes() {
        let mut b = book();
        b.recipes
            .get_mut("Bench_Recipe")
            .unwrap()
            .bench_requirement
            .as_mut()
            .unwrap()[0]
            .id = "Workbench".into();
        let bench = Bench {
            kind: 0,
            id: "Workbench".into(),
            categories: vec![("Tools".into(), "i.png".into(), "n".into())],
            ..Default::default()
        };
        let w = b
            .bench_window(1, "Bench_WorkBench", (0, 0, 0), &bench)
            .unwrap();
        let d = w.data.text();
        assert!(d.contains("\"craftableRecipes\":[\"Bench_Recipe\"]"), "{d}");
        assert!(d.starts_with("{\"type\":0,\"id\":\"Workbench\""), "{d}");
        let mut data = w.data.clone();
        data.set("type", "5".into());
        assert!(data.text().starts_with("{\"type\":5,"));
    }
}
