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
use crate::messages::Msg;

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
    /// Where sorting puts it (spec 14): 0 weapon, 1 armor, 2 tool, 3 plain,
    /// 4 builder tool.
    sort_rank: u8,
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
    /// Item → the recipe the official server finds for it when it tells a
    /// client the item is known (see `incoming_recipes`).
    incoming: HashMap<String, String>,
    /// (bench id, category) → its recipes in the order the official
    /// registry's set lists them (what a bench window shows).
    in_category: HashMap<(String, String), Vec<String>>,
    /// Item → the item its `LearnRecipe` interaction teaches.
    learns: HashMap<String, String>,
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
                                    sort_rank: if it.weapon.is_some() {
                                        0
                                    } else if it.armor.is_some() {
                                        1
                                    } else if it.tool.is_some() {
                                        2
                                    } else if it.builder_tool_data.is_some() {
                                        4
                                    } else {
                                        3
                                    },
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
                ["L", item, learned] => {
                    book.learns.insert(item.to_owned(), learned.to_owned());
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
        (book.incoming, book.in_category) = book.registries();
        book
    }

    /// The recipe the official server finds for each item a client learns
    /// it knows. Each bench id has a registry of the recipes that name it,
    /// by category (or none); for each item the registry lists the recipes
    /// that make it. The server goes through every registry (in its map's
    /// order) and every recipe listed for the item (in its set's order),
    /// and the last one stays. All those sets and maps are open addressing
    /// tables, so their order is the order of their hashes, and the order
    /// the recipes were added in settles ties (taken here as the recipes'
    /// names in order).
    #[allow(clippy::type_complexity)]
    fn registries(
        &self,
    ) -> (
        HashMap<String, String>,
        HashMap<(String, String), Vec<String>>,
    ) {
        struct Registry {
            categories: Open,
            by_category: HashMap<String, Open>,
            loose: Open,
        }
        let mut ids: Vec<&String> = self.recipes.keys().collect();
        ids.sort_by(|a, b| b.cmp(a));
        let mut benches = Open::made_for(16);
        let mut registries: HashMap<String, Registry> = HashMap::new();
        for id in ids {
            for req in self.recipes[id].bench_requirement.iter().flatten() {
                benches.add(&req.id);
                let reg = registries
                    .entry(req.id.clone())
                    .or_insert_with(|| Registry {
                        categories: Open::made_for(16),
                        by_category: HashMap::new(),
                        loose: Open::made_for(16),
                    });
                match req.categories.as_deref() {
                    Some(c) if !c.is_empty() => {
                        for c in c {
                            reg.categories.add(c);
                            reg.by_category
                                .entry(c.clone())
                                .or_insert_with(|| Open::made_for(16))
                                .add(id);
                        }
                    }
                    _ => reg.loose.add(id),
                }
            }
        }
        let mut found = HashMap::new();
        let mut listed_in = HashMap::new();
        for bench in benches.order() {
            let reg = &registries[&bench];
            let mut listed: HashMap<&str, Open> = HashMap::new();
            for (c, set) in &reg.by_category {
                listed_in.insert((bench.clone(), c.clone()), set.order());
            }
            let in_order = reg
                .categories
                .order()
                .into_iter()
                .flat_map(|c| reg.by_category[&c].order())
                .chain(reg.loose.order());
            for id in in_order {
                for item in self.recipes[&id]
                    .outputs
                    .iter()
                    .flatten()
                    .filter_map(|o| o.item_id.as_deref())
                {
                    listed
                        .entry(item)
                        .or_insert_with(|| Open::made_for(16))
                        .add(&id);
                }
            }
            for (item, set) in listed {
                if let Some(last) = set.order().pop() {
                    found.insert(item.to_owned(), last);
                }
            }
        }
        (found, listed_in)
    }

    /// What the client is told it knows (`UpdateKnownRecipes`): for each
    /// learned item that exists and that some bench makes, the recipe
    /// that makes it, under the item's id. The official server fills a
    /// hash map with them, so the entries come in that table's order.
    pub fn known_recipes(&self, known: &Known) -> Vec<(String, proto::CraftingRecipe)> {
        let mut table = Open::made_for(16);
        let mut found: HashMap<&str, proto::CraftingRecipe> = HashMap::new();
        for item in known.ids() {
            if !self.items.contains_key(item) {
                continue;
            }
            let Some(recipe) = self.incoming.get(item).and_then(|r| self.recipes.get(r)) else {
                continue;
            };
            let mut recipe = recipe.clone();
            recipe.id = Some(item.clone());
            found.insert(item, recipe);
            table.add(item);
        }
        table
            .order()
            .into_iter()
            .filter_map(|k| found.remove(k.as_str()).map(|r| (k, r)))
            .collect()
    }

    /// The item that using `item` teaches (its `LearnRecipe` step).
    pub fn learns(&self, item: &str) -> Option<&str> {
        self.learns.get(item).map(String::as_str)
    }

    /// Whether the player may craft `recipe`: known when it needs
    /// knowledge, by its primary output's item.
    pub fn knows(&self, known: &Known, recipe: &str) -> bool {
        let Some(r) = self.recipes.get(recipe) else {
            return false;
        };
        !r.knowledge_required
            || r.primary_output
                .as_ref()
                .and_then(|p| p.item_id.as_deref())
                .is_some_and(|i| known.contains(i))
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

    /// Where sorting puts an item (spec 14); 3 for one Aerie does not know.
    pub fn sort_rank(&self, item: &str) -> u8 {
        self.items.get(item).map_or(3, |i| i.sort_rank)
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

    /// Whether there is such an item (anything goes without the item
    /// registry, as in tests).
    pub fn known(&self, item: &str) -> bool {
        self.items.is_empty() || self.items.contains_key(item)
    }

    /// Every item key, sorted.
    pub fn item_keys(&self) -> Vec<String> {
        let mut v: Vec<String> = self.items.keys().cloned().collect();
        v.sort();
        v
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
        self.in_category
            .get(&(id.to_owned(), cat.to_owned()))
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default()
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
    pub fn allowed(&self, id: &str, kind: i32, bench: &str, known: &Known) -> Result<(), Refused> {
        let r = self.recipes.get(id).ok_or(Refused::Unknown)?;
        let at_bench = r.bench_requirement.iter().flatten().any(|b| {
            b.r#type.0 == kind
                && b.id == bench
                && b.required_tier_level <= 1
                && b.required_augment_tags
                    .as_ref()
                    .is_none_or(|t| t.is_empty())
        });
        if !at_bench || !self.knows(known, id) || r.required_memories_level > MEMORIES_LEVEL {
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

// ---------------------------------------------------------------------
// Known recipes (spec 09)

/// `String.hashCode` over UTF-16 code units.
fn java_hash(s: &str) -> u32 {
    s.encode_utf16()
        .fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32))
}

/// The open addressing table the official server keeps its sets and maps
/// in (mixed hash, linear probing, growing at 3/4). Only the order it
/// reads its keys in matters here: from the last slot down.
struct Open {
    keys: Vec<Option<String>>,
    size: usize,
    /// The size it was made with: it never shrinks below it.
    least: usize,
}

impl Open {
    fn table_size(expected: usize) -> usize {
        let need = (expected as f64 / 0.75).ceil() as usize;
        need.next_power_of_two().max(2)
    }

    fn made_for(expected: usize) -> Self {
        let n = Self::table_size(expected);
        Open {
            keys: vec![None; n],
            size: 0,
            least: n,
        }
    }

    fn slot(&self, key: &str) -> usize {
        let h = java_hash(key).wrapping_mul(0x9E37_79B9);
        ((h ^ (h >> 16)) as usize) & (self.keys.len() - 1)
    }

    fn max_fill(n: usize) -> usize {
        ((n as f64 * 0.75).ceil() as usize).min(n - 1)
    }

    fn rehash(&mut self, n: usize) {
        let old = std::mem::replace(&mut self.keys, vec![None; n]);
        for k in old.into_iter().rev().flatten() {
            let mut pos = self.slot(&k);
            while self.keys[pos].is_some() {
                pos = (pos + 1) & (n - 1);
            }
            self.keys[pos] = Some(k);
        }
    }

    fn add(&mut self, key: &str) {
        let mask = self.keys.len() - 1;
        let mut pos = self.slot(key);
        while let Some(k) = &self.keys[pos] {
            if k == key {
                return;
            }
            pos = (pos + 1) & mask;
        }
        self.keys[pos] = Some(key.to_owned());
        if self.size >= Self::max_fill(self.keys.len()) {
            self.size += 1;
            self.rehash(Self::table_size(self.size + 1));
        } else {
            self.size += 1;
        }
    }

    fn remove(&mut self, key: &str) {
        let n = self.keys.len();
        let mask = n - 1;
        let mut pos = self.slot(key);
        loop {
            match &self.keys[pos] {
                None => return,
                Some(k) if k == key => break,
                _ => pos = (pos + 1) & mask,
            }
        }
        // Close the gap: later keys of the run move back when their
        // home slot allows it.
        loop {
            let last = pos;
            pos = (last + 1) & mask;
            loop {
                let Some(k) = &self.keys[pos] else {
                    self.keys[last] = None;
                    self.size -= 1;
                    if n > self.least && self.size < Self::max_fill(n) / 4 && n > 16 {
                        self.rehash(n / 2);
                    }
                    return;
                };
                let home = self.slot(k);
                let stays = if last <= pos {
                    last >= home || home > pos
                } else {
                    last >= home && home > pos
                };
                if stays {
                    break;
                }
                pos = (pos + 1) & mask;
            }
            self.keys[last] = self.keys[pos].take();
        }
    }

    fn order(&self) -> Vec<String> {
        self.keys.iter().rev().flatten().cloned().collect()
    }
}

/// The order a Java `HashSet` iterates `prev` copied into a new set that
/// then gets `add`: a table made for the copy, buckets of `h ^ h >>> 16`,
/// insertion order inside a bucket (a resize keeps it).
fn hash_set_order(prev: &[String], add: &str) -> Vec<String> {
    let mut cap = (((prev.len() as f32 / 0.75) as usize) + 1)
        .max(16)
        .next_power_of_two();
    if prev.len() + 1 > (cap as f32 * 0.75) as usize {
        cap *= 2;
    }
    let mut v: Vec<(usize, usize, &str)> = prev
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(add))
        .enumerate()
        .map(|(i, s)| {
            let h = java_hash(s);
            (((h ^ (h >> 16)) as usize) & (cap - 1), i, s)
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, _, s)| s.to_owned()).collect()
}

/// The recipes a player has learned (spec 09), kept in the order the
/// official server's set lists them: `/recipe list` prints that order.
/// A set made by learning is a hash set, one made by forgetting is an
/// open addressing set, and what was saved comes back in the saved order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Known {
    ids: Vec<String>,
}

impl Known {
    /// From the saved text: one item id per line.
    pub fn parse(text: &str) -> Self {
        let mut ids: Vec<String> = Vec::new();
        for l in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !ids.iter().any(|i| i == l) {
                ids.push(l.to_owned());
            }
        }
        Known { ids }
    }

    pub fn text(&self) -> String {
        self.ids.iter().map(|i| format!("{i}\n")).collect()
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn contains(&self, item: &str) -> bool {
        self.ids.iter().any(|i| i == item)
    }

    /// Learns `item`; false when it was known already.
    pub fn learn(&mut self, item: &str) -> bool {
        if self.contains(item) {
            return false;
        }
        self.ids = hash_set_order(&self.ids, item);
        true
    }

    /// Forgets `item`; false when it was not known.
    pub fn forget(&mut self, item: &str) -> bool {
        if !self.contains(item) {
            return false;
        }
        let mut set = Open::made_for(self.ids.len());
        for i in &self.ids {
            set.add(i);
        }
        set.remove(item);
        self.ids = set.order();
        true
    }

    /// What `/recipe list` prints: the ids in brackets.
    pub fn list(&self) -> String {
        format!("[{}]", self.ids.join(", "))
    }
}

/// What a `LearnRecipe` step answers when the held item teaches `item`
/// (none: the item names nothing to learn), and whether `known` changed.
pub fn learn_reply(book: &Book, known: &mut Known, item: Option<&str>) -> (Msg, bool) {
    let Some(item) = item else {
        return (Msg::key("server.modules.learnrecipe.noIdSet"), false);
    };
    let name = Msg::key(&book.item_name(item));
    if known.learn(item) {
        (
            Msg::key("server.modules.learnrecipe.success").msg("name", name),
            true,
        )
    } else {
        (
            Msg::key("server.modules.learnrecipe.alreadyKnown").msg("name", name),
            false,
        )
    }
}

/// What `/recipe` answers (and whether `known` changed): `sub` is `learn`,
/// `forget` or `list`, `item` the item key for the first two, `other` the
/// target's name when the command names another player (the replies then
/// say who).
pub fn recipe_reply(
    book: &Book,
    known: &mut Known,
    sub: &str,
    item: &str,
    other: Option<&str>,
) -> (Msg, bool) {
    const GREEN: &str = "#00ff00";
    const RED: &str = "#ff0000";
    let name = || Msg::key(&book.item_name(item));
    let who = |m: Msg| match other {
        Some(u) => m.text("username", u),
        None => m,
    };
    let key = |own: &str, others: &str| if other.is_some() { others } else { own }.to_owned();
    match sub {
        "learn" => {
            if known.learn(item) {
                let k = key(
                    "server.modules.learnrecipe.success",
                    "server.commands.recipe.learn.success.other",
                );
                (who(Msg::key(&k)).msg("name", name()).color(GREEN), true)
            } else {
                let k = key(
                    "server.modules.learnrecipe.alreadyKnown",
                    "server.commands.recipe.learn.alreadyKnown.other",
                );
                (who(Msg::key(&k)).msg("name", name()).color(RED), false)
            }
        }
        "forget" => {
            if known.forget(item) {
                let k = key(
                    "server.commands.recipe.forgotten",
                    "server.commands.recipe.forgotten.other",
                );
                (who(Msg::key(&k)).text("id", item).color(GREEN), true)
            } else {
                let k = key(
                    "server.commands.recipe.alreadyNotKnown",
                    "server.commands.recipe.alreadyNotKnown.other",
                );
                (who(Msg::key(&k)).text("id", item).color(RED), false)
            }
        }
        _ => {
            let k = key(
                "server.commands.recipe.knownRecipes",
                "server.commands.recipe.knownRecipes.other",
            );
            (who(Msg::key(&k)).text("list", &known.list()), false)
        }
    }
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
            b.allowed("Bench_Recipe", 0, "Workbench", &Known::default()),
            Err(Refused::WrongBench)
        );
        assert_eq!(
            b.allowed("Nope", 0, FIELDCRAFT, &Known::default()),
            Err(Refused::Unknown)
        );
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
        (b.incoming, b.in_category) = b.registries();
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

    #[test]
    fn known_recipes_list_in_the_official_order() {
        let mut k = Known::default();
        assert_eq!(k.list(), "[]");
        for i in ["Food_Pie_Apple", "Weapon_Sword_Iron", "Rock_Stone"] {
            assert!(k.learn(i));
        }
        assert!(!k.learn("Rock_Stone"));
        assert!(k.forget("Food_Pie_Apple"));
        assert!(!k.forget("Food_Pie_Apple"));
        assert!(k.learn("Food_Pie_Apple"));
        assert!(k.learn("Tool_Pickaxe_Iron"));
        assert_eq!(
            k.list(),
            "[Tool_Pickaxe_Iron, Food_Pie_Apple, Weapon_Sword_Iron, Rock_Stone]"
        );
        // Seventy-one learned in a row (the set grows twice), as the
        // official server listed them.
        let learned = [
            "Container_Bucket",
            "Deco_Book_Pile_Large",
            "Deco_Tankard",
            "Food_Cheese",
            "Food_Fish_Raw",
            "Ingredient_Bar_Iron",
            "Ingredient_Bolt_Cindercloth",
            "Ingredient_Bolt_Cotton",
            "Ingredient_Bolt_Linen",
            "Ingredient_Bolt_Silk",
            "Ingredient_Bolt_Wool",
            "Ingredient_Bone_Fragment",
            "Ingredient_Fabric_Scrap_Cindercloth",
            "Ingredient_Fabric_Scrap_Linen",
            "Ingredient_Fabric_Scrap_Shadoweave",
            "Ingredient_Feathers_Dark",
            "Ingredient_Fibre",
            "Ingredient_Gadget_Assembly",
            "Ingredient_Hide_Heavy",
            "Ingredient_Hide_Light",
            "Ingredient_Hide_Medium",
            "Ingredient_Hide_Storm",
            "Ingredient_Life_Essence",
            "Ingredient_Powder_Boom",
            "Ingredient_Sac_Venom",
            "Ingredient_Scraps",
            "Ingredient_Stick",
            "Ingredient_Tree_Sap",
            "Ingredient_Void_Essence",
            "Ore_Adamantite",
            "Ore_Cobalt",
            "Ore_Copper",
            "Ore_Iron",
            "Ore_Mithril",
            "Ore_Thorium",
            "Plant_Petals_Azure",
            "Plant_Petals_Black",
            "Plant_Petals_Blood",
            "Plant_Petals_Blue",
            "Plant_Petals_Cyan",
            "Plant_Petals_Green",
            "Plant_Petals_Orange",
            "Plant_Petals_Pink",
            "Plant_Petals_Purple",
            "Plant_Petals_Red",
            "Plant_Petals_Storm",
            "Plant_Petals_White",
            "Plant_Petals_Yellow",
            "Potion_Health",
            "Rock_Stone_Cobble",
            "Rubble_Aqua",
            "Rubble_Basalt",
            "Rubble_Calcite",
            "Rubble_Lime",
            "Rubble_Marble",
            "Food_Pie_Apple",
            "Weapon_Sword_Iron",
            "Tool_Pickaxe_Iron",
            "Rock_Stone",
            "Plant_Fruit_Apple",
            "Ingredient_Dough",
            "Furniture_Crude_Chest_Small",
            "Bench_WorkBench",
            "Weapon_Shortbow_Iron",
            "Armor_Iron_Chest",
            "Ingredient_Bar_Copper",
            "Ingredient_Bar_Gold",
            "Food_Bread",
            "Potion_Health_Small",
            "Tool_Hatchet_Iron",
            "Weapon_Axe_Iron",
        ];
        let listed = [
            "Ingredient_Bolt_Cindercloth",
            "Ingredient_Hide_Heavy",
            "Armor_Iron_Chest",
            "Plant_Petals_Pink",
            "Ingredient_Hide_Storm",
            "Rubble_Marble",
            "Furniture_Crude_Chest_Small",
            "Rubble_Calcite",
            "Ingredient_Hide_Medium",
            "Food_Bread",
            "Rubble_Aqua",
            "Ingredient_Fabric_Scrap_Cindercloth",
            "Deco_Book_Pile_Large",
            "Ingredient_Fabric_Scrap_Linen",
            "Ore_Adamantite",
            "Rubble_Basalt",
            "Ingredient_Dough",
            "Bench_WorkBench",
            "Ingredient_Bar_Copper",
            "Ingredient_Feathers_Dark",
            "Ingredient_Void_Essence",
            "Rubble_Lime",
            "Ingredient_Life_Essence",
            "Food_Pie_Apple",
            "Ingredient_Sac_Venom",
            "Weapon_Sword_Iron",
            "Ingredient_Hide_Light",
            "Plant_Petals_Red",
            "Container_Bucket",
            "Tool_Hatchet_Iron",
            "Plant_Petals_Purple",
            "Ingredient_Fibre",
            "Ore_Cobalt",
            "Potion_Health",
            "Ore_Mithril",
            "Ingredient_Scraps",
            "Potion_Health_Small",
            "Ore_Thorium",
            "Ingredient_Bolt_Wool",
            "Ingredient_Tree_Sap",
            "Deco_Tankard",
            "Ingredient_Gadget_Assembly",
            "Plant_Fruit_Apple",
            "Rock_Stone_Cobble",
            "Plant_Petals_Orange",
            "Ingredient_Fabric_Scrap_Shadoweave",
            "Ingredient_Bone_Fragment",
            "Ore_Copper",
            "Plant_Petals_Azure",
            "Ingredient_Stick",
            "Plant_Petals_Cyan",
            "Food_Fish_Raw",
            "Ingredient_Bolt_Linen",
            "Ingredient_Bolt_Silk",
            "Ingredient_Bar_Gold",
            "Weapon_Axe_Iron",
            "Plant_Petals_Blood",
            "Plant_Petals_Blue",
            "Plant_Petals_Green",
            "Plant_Petals_White",
            "Plant_Petals_Black",
            "Food_Cheese",
            "Ingredient_Powder_Boom",
            "Plant_Petals_Storm",
            "Ingredient_Bolt_Cotton",
            "Rock_Stone",
            "Plant_Petals_Yellow",
            "Weapon_Shortbow_Iron",
            "Tool_Pickaxe_Iron",
            "Ingredient_Bar_Iron",
            "Ore_Iron",
        ];
        let mut k = Known::default();
        for i in learned {
            k.learn(i);
        }
        assert_eq!(k.ids(), listed);
        assert_eq!(Known::parse(&k.text()), k);
    }
}
