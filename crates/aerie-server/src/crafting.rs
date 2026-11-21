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
    /// (id, icon, name) in order.
    pub categories: Vec<(String, String, String)>,
}

/// A crafting window a player has open.
#[derive(Clone, Debug)]
pub struct Window {
    pub id: i32,
    pub bench: Bench,
    /// The bench block's key and position; none for the pocket window.
    pub block: Option<(String, (i32, i32, i32))>,
}

impl Window {
    pub fn fieldcraft() -> Self {
        Window {
            id: 0,
            bench: Bench {
                kind: proto::BenchType::CRAFTING.0,
                id: FIELDCRAFT.into(),
                ..Default::default()
            },
            block: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ItemInfo {
    resources: Vec<(String, i32)>,
    tags: Vec<i32>,
    name: Option<String>,
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
}

/// Why a craft did not happen.
#[derive(Debug, PartialEq)]
pub enum Refused {
    /// Unknown recipe: the official server disconnects.
    Unknown,
    /// Not made at this bench (or needs knowledge, memories, augments).
    WrongBench,
    Missing,
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
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        for l in benches.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w[..] {
                ["W", block, kind, id, open, close, completed] => {
                    book.benches.insert(
                        block.to_owned(),
                        Bench {
                            kind: kind.parse().unwrap_or(0),
                            id: id.to_owned(),
                            open_sound: open.parse().unwrap_or(0),
                            close_sound: close.parse().unwrap_or(0),
                            completed_sound: completed.parse().unwrap_or(0),
                            categories: Vec::new(),
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
                _ => {}
            }
        }
        book
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.recipes.len(), self.items.len(), self.benches.len())
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

    /// `window_data` of the pocket window, as the official server writes it
    /// (its category list stays empty).
    pub fn fieldcraft_data(&self) -> String {
        format!(
            "{{\"type\":{},\"id\":{},\"name\":{},\"categories\":[],\"worldMemoriesLevel\":{MEMORIES_LEVEL}}}",
            proto::BenchType::CRAFTING.0,
            json(FIELDCRAFT),
            json("client.inventory.fieldcraft.title"),
        )
    }

    /// `window_data` of a bench window for the bench block `block`.
    pub fn bench_data(&self, block: &str, bench: &Bench) -> String {
        let name = self
            .items
            .get(block)
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| format!("server.items.{block}.name"));
        let cats: Vec<String> = bench
            .categories
            .iter()
            .map(|(id, icon, n)| {
                let recipes: Vec<String> = self
                    .recipes_for(&bench.id, id)
                    .iter()
                    .map(|r| json(r))
                    .collect();
                format!(
                    "{{\"id\":{},\"name\":{},\"icon\":{},\"craftableRecipes\":[{}]}}",
                    json(id),
                    json(n),
                    json(icon),
                    recipes.join(",")
                )
            })
            .collect();
        let per: Vec<String> = self
            .memories_per_level
            .iter()
            .map(|n| n.to_string())
            .collect();
        format!(
            "{{\"type\":{},\"id\":{},\"name\":{},\"blockItemId\":{},\"tierLevel\":1,\
             \"grantedAugmentTags\":[],\"categories\":[{}],\"worldMemoriesLevel\":{MEMORIES_LEVEL},\
             \"nearbyChestCount\":0,\"maxChestCount\":{MAX_CHEST_COUNT},\
             \"chestHorizontalRadius\":{CHEST_HORIZONTAL_RADIUS},\
             \"chestVerticalRadius\":{CHEST_VERTICAL_RADIUS},\"memoriesPerLevel\":[{}]}}",
            bench.kind,
            json(&bench.id),
            json(&name),
            json(block),
            cats.join(","),
            per.join(",")
        )
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

    /// Makes `quantity` of recipe `id` at bench (`kind`, `bench`): takes the
    /// inputs and returns the outputs to give (item, count).
    pub fn craft(
        &self,
        inv: &mut Inventory,
        id: &str,
        quantity: i32,
        kind: i32,
        bench: &str,
        creative: bool,
    ) -> Result<Vec<(String, i32)>, Refused> {
        let r = self.recipes.get(id).ok_or(Refused::Unknown)?;
        let quantity = quantity.max(1);
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
        if !creative {
            let mut after = inv.clone();
            for m in r.inputs.iter().flatten() {
                let left = after.take_units(|item| self.units(m, item), m.quantity * quantity);
                if left > 0 {
                    return Err(Refused::Missing);
                }
            }
            *inv = after;
        }
        Ok(r.outputs
            .iter()
            .flatten()
            .filter_map(|m| Some((m.item_id.clone()?, m.quantity * quantity)))
            .collect())
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
    fn crafts_with_resources_and_items() {
        let b = book();
        let mut inv = Inventory::default();
        inv.add("Log", 5);
        inv.add("Rock", 2);
        assert_eq!(
            b.craft(&mut inv, "Bench_Recipe", 1, 0, FIELDCRAFT, false),
            Err(Refused::Missing)
        );
        assert_eq!(inv.get(-1, 0), Some(("Log".into(), 5)), "nothing taken");
        inv.add("Rock", 1);
        assert_eq!(
            b.craft(&mut inv, "Bench_Recipe", 1, 0, FIELDCRAFT, false),
            Ok(vec![("Bench".into(), 1)])
        );
        assert_eq!(inv.get(-1, 0), Some(("Log".into(), 1)));
        assert_eq!(inv.get(-1, 1), None);
        assert_eq!(
            b.craft(&mut inv, "Bench_Recipe", 1, 0, "Workbench", true),
            Err(Refused::WrongBench)
        );
        assert_eq!(
            b.craft(&mut inv, "Nope", 1, 0, FIELDCRAFT, true),
            Err(Refused::Unknown)
        );
    }

    #[test]
    fn bench_data_lists_recipes() {
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
        let d = b.bench_data("Bench_WorkBench", &bench);
        assert!(d.contains("\"craftableRecipes\":[\"Bench_Recipe\"]"), "{d}");
        assert!(d.starts_with("{\"type\":0,\"id\":\"Workbench\""), "{d}");
    }
}
