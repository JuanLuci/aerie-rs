//! Block damage from hits (spec 12): which spec a hit uses, how much it
//! takes, what it drops, the tool's sounds and wear. Read from the
//! `gathering` table (aerie-assets).

use std::collections::HashMap;

pub const TABLE: &str = "gathering";

/// A block's `Gathering`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub gather: Option<String>,
    pub quality: i32,
    pub item: Option<String>,
    pub list: Option<String>,
    pub quantity: i32,
    pub soft: bool,
    pub soft_item: Option<String>,
    pub soft_list: Option<String>,
    pub soft_weapon: bool,
    pub harvest: bool,
    pub use_default_drop: bool,
    /// `Gathering.Tools`: what breaking it with a tool type does.
    pub tools: Vec<ToolData>,
}

/// One `Gathering.Tools` entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolData {
    pub tool: String,
    pub state: Option<String>,
    pub item: Option<String>,
    pub list: Option<String>,
}

/// A tool spec (or a bare-hand one).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spec {
    pub gather: String,
    pub power: f32,
    pub quality: i32,
    pub incorrect: bool,
    pub hit_sound: i32,
}

/// An item that is a tool, a weapon or a builder tool.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub weapon: bool,
    pub builder: bool,
    pub tool: bool,
    pub specs: Vec<Spec>,
    pub hit_sound: i32,
    pub incorrect_sound: i32,
    pub max_durability: f64,
    pub loss_on_hit: f64,
    /// (loss, block sets, block keys).
    pub loss_for: Vec<(f64, Vec<String>, Vec<String>)>,
}

/// A gathering effect (particle system, sound).
#[derive(Clone, Debug, Default)]
pub struct Effect {
    pub particle: Option<String>,
    pub sound: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    blocks: HashMap<String, Block>,
    items: HashMap<String, Item>,
    unarmed: HashMap<String, Spec>,
    /// (item, interaction type) → the break tool its interactions run and
    /// whether it must match.
    break_tools: HashMap<(String, String), (Option<String>, bool)>,
    pub unbreakable: Effect,
    pub incorrect: Effect,
    /// The gameplay config's broken tool penalty (0.75: a quarter of the
    /// power left).
    pub broken_tool_penalty: f64,
}

/// What one hit does (spec 12).
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    /// No gathering, or a weapon on a block that only tools break.
    Nothing,
    /// A held item that is no weapon cannot break it: the unbreakable
    /// effect.
    Unbreakable,
    /// Damage, and what breaking it then drops.
    Damage {
        damage: f32,
        item: Option<String>,
        list: Option<String>,
        quantity: i32,
        spec: Option<Spec>,
        /// The block's entry for the break tool, if it has one.
        tool: Option<ToolData>,
    },
}

fn opt(s: &str) -> Option<String> {
    (s != "-").then(|| s.to_owned())
}

fn csv(s: &str) -> Vec<String> {
    if s == "-" {
        Vec::new()
    } else {
        s.split(',').map(str::to_owned).collect()
    }
}

impl Table {
    pub fn parse(text: &str) -> Self {
        let mut t = Table::default();
        for l in text.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w[..] {
                [
                    "B",
                    key,
                    gather,
                    quality,
                    item,
                    list,
                    qty,
                    soft,
                    soft_item,
                    soft_list,
                    soft_weapon,
                    harvest,
                    _,
                    _,
                    use_default,
                ] => {
                    t.blocks.insert(
                        key.to_owned(),
                        Block {
                            gather: opt(gather),
                            quality: quality.parse().unwrap_or(0),
                            item: opt(item),
                            list: opt(list),
                            quantity: qty.parse().unwrap_or(1),
                            soft: soft == "1",
                            soft_item: opt(soft_item),
                            soft_list: opt(soft_list),
                            soft_weapon: soft_weapon == "1",
                            harvest: harvest == "1",
                            use_default_drop: use_default == "1",
                            tools: Vec::new(),
                        },
                    );
                }
                ["G", key, tool, state, item, list] => {
                    if let Some(b) = t.blocks.get_mut(key) {
                        b.tools.push(ToolData {
                            tool: tool.to_owned(),
                            state: opt(state),
                            item: opt(item),
                            list: opt(list),
                        });
                    }
                }
                ["K", item, ty, tool, m] => {
                    t.break_tools
                        .insert((item.to_owned(), ty.to_owned()), (opt(tool), m == "1"));
                }
                ["I", key, weapon, builder, tool, hit, incorrect, max, loss] => {
                    let it = t.items.entry(key.to_owned()).or_default();
                    it.weapon = weapon == "1";
                    it.builder = builder == "1";
                    it.tool = tool == "1";
                    it.hit_sound = hit.parse().unwrap_or(0);
                    it.incorrect_sound = incorrect.parse().unwrap_or(0);
                    it.max_durability = max.parse().unwrap_or(0.0);
                    it.loss_on_hit = loss.parse().unwrap_or(0.0);
                }
                ["T", key, gather, power, quality, incorrect, hit] => {
                    t.items.entry(key.to_owned()).or_default().specs.push(Spec {
                        gather: gather.to_owned(),
                        power: power.parse().unwrap_or(0.0),
                        quality: quality.parse().unwrap_or(0),
                        incorrect: incorrect == "1",
                        hit_sound: hit.parse().unwrap_or(0),
                    });
                }
                ["L", key, loss, sets, types] => {
                    t.items.entry(key.to_owned()).or_default().loss_for.push((
                        loss.parse().unwrap_or(0.0),
                        csv(sets),
                        csv(types),
                    ));
                }
                ["U", gather, power, quality, hit] => {
                    t.unarmed.insert(
                        gather.to_owned(),
                        Spec {
                            gather: gather.to_owned(),
                            power: power.parse().unwrap_or(0.0),
                            quality: quality.parse().unwrap_or(0),
                            incorrect: false,
                            hit_sound: hit.parse().unwrap_or(0),
                        },
                    );
                }
                ["X", "tool", v] => t.broken_tool_penalty = v.parse().unwrap_or(0.0),
                ["E", kind, particle, sound] => {
                    let e = Effect {
                        particle: opt(particle),
                        sound: sound.parse().unwrap_or(0),
                    };
                    match kind {
                        "unbreakable" => t.unbreakable = e,
                        "incorrect" => t.incorrect = e,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        t
    }

    /// The break tool a held item's interactions of a type run (none, and
    /// no match needed, for bare hands and items without one).
    pub fn break_tool(&self, held: Option<&str>, ty: &str) -> (Option<String>, bool) {
        held.and_then(|h| self.break_tools.get(&(h.to_owned(), ty.to_owned())))
            .cloned()
            .unwrap_or((None, false))
    }

    pub fn item(&self, key: &str) -> Option<&Item> {
        self.items.get(key)
    }

    /// The spec a hit uses (the official choice): none for weapons and
    /// builder tools; without a tool the bare hand's spec of the block's
    /// gather type; with one the first spec of that gather type; none if the
    /// spec's quality is below the block's.
    pub fn spec(&self, block: &Block, held: Option<&str>) -> Option<Spec> {
        let gather = block.gather.as_ref()?;
        let item = held.and_then(|h| self.items.get(h));
        if item.is_some_and(|i| i.weapon || i.builder) {
            return None;
        }
        match item.filter(|i| i.tool) {
            None => {
                let s = self.unarmed.get(gather)?;
                (s.quality >= block.quality).then(|| s.clone())
            }
            Some(i) => {
                let s = i.specs.iter().find(|s| &s.gather == gather)?;
                (s.quality >= block.quality).then(|| s.clone())
            }
        }
    }

    /// What a hit on `key` with `held` does (spec 12); `deco`: the block was
    /// placed as decoration (its default drop then); `broken`: the held
    /// stack is worn out (a tool then hits with the penalty).
    #[cfg(test)]
    pub fn hit(&self, key: &str, held: Option<&str>, deco: bool, broken: bool) -> Hit {
        self.hit_with(key, held, deco, broken, None, false)
    }

    /// [`Self::hit`] with the break tool the interaction runs: a tool that
    /// must match does nothing on a block without an entry for it.
    pub fn hit_with(
        &self,
        key: &str,
        held: Option<&str>,
        deco: bool,
        broken: bool,
        tool: Option<&str>,
        match_tool: bool,
    ) -> Hit {
        let Some(b) = self.blocks.get(key) else {
            return Hit::Nothing;
        };
        let tool_data = tool
            .and_then(|t| b.tools.iter().find(|d| d.tool == t))
            .cloned();
        if match_tool && tool_data.is_none() {
            return Hit::Nothing;
        }
        let spec = self.spec(b, held);
        let mut power = spec.as_ref().map_or(0.0, |s| s.power);
        if power != 0.0 && broken && held.and_then(|h| self.items.get(h)).is_some_and(|i| i.tool) {
            power *= 1.0 - self.broken_tool_penalty as f32;
        }
        let (damage, mut item, mut list, quantity) = if power != 0.0 {
            (power, b.item.clone(), b.list.clone(), b.quantity)
        } else {
            if !b.soft {
                return match held {
                    Some(h) if !self.items.get(h).is_some_and(|i| i.weapon) => Hit::Unbreakable,
                    _ => Hit::Nothing,
                };
            }
            if !b.soft_weapon
                && held
                    .and_then(|h| self.items.get(h))
                    .is_some_and(|i| i.weapon)
            {
                return Hit::Nothing;
            }
            (1.0, b.soft_item.clone(), b.soft_list.clone(), 1)
        };
        if b.use_default_drop && deco {
            item = None;
            list = None;
        }
        Hit::Damage {
            damage,
            item,
            list,
            quantity,
            spec,
            tool: tool_data,
        }
    }
}

impl Table {
    /// Durability a damaging hit on `key` (block id `id`) takes from
    /// `held` (the official rule): none on soft blocks, blocks without
    /// gathering or for items that are no tool; else the first
    /// `DurabilityLossBlockTypes` entry naming the block, or a set it is in
    /// (`in_set`), else the item's `DurabilityLossOnHit`.
    pub fn wear(&self, held: &str, key: &str, in_set: impl Fn(&str) -> bool) -> f64 {
        let Some(b) = self.blocks.get(key) else {
            return 0.0;
        };
        let Some(i) = self.items.get(held).filter(|i| i.tool) else {
            return 0.0;
        };
        if b.soft {
            return 0.0;
        }
        for (loss, sets, types) in &i.loss_for {
            if types.iter().any(|t| t == key) || sets.iter().any(|s| in_set(s)) {
                return *loss;
            }
        }
        i.loss_on_hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "B Wood_Oak_Trunk Woods 0 Wood_Oak_Trunk - 1 0 - - 1 0 - - 1\n\
                     B Rock_Stone Rocks 0 Rock_Stone_Cobble - 1 0 - - 1 0 - - 0\n\
                     B Plant_Grass - 0 - - 1 1 - Grass_Drops 1 0 - - 0\n\
                     I Tool_Hatchet_Crude 0 0 1 0 0 200 0\n\
                     T Tool_Hatchet_Crude Woods 0.15 0 0 0\n\
                     T Tool_Hatchet_Crude Metals 0.001 0 1 7\n\
                     I Weapon_Sword 1 0 0 0 0 100 0\n\
                     U Woods 0.05 0 3\n\
                     L Tool_Hatchet_Crude 0.25 Stone,Wood -\n\
                     X tool 0.75\n\
                     G Wood_Oak_Trunk Scraper Stripped Bark -\n\
                     G Plant_Grass Shears - - -\n\
                     K Tool_Scraper Primary Scraper 1\n\
                     K Tool_Shears Primary Shears 0\n\
                     U Rocks 0.02 0 0\n";

    #[test]
    fn hands_tools_weapons_soft() {
        let t = Table::parse(T);
        match t.hit("Wood_Oak_Trunk", None, false, false) {
            Hit::Damage { damage, item, .. } => {
                assert_eq!(damage, 0.05);
                assert_eq!(item.as_deref(), Some("Wood_Oak_Trunk"));
            }
            h => panic!("{h:?}"),
        }
        assert!(
            matches!(t.hit("Wood_Oak_Trunk", Some("Tool_Hatchet_Crude"), false, false), Hit::Damage { damage, .. } if damage == 0.15)
        );
        // A tool without a spec for the block: cannot break it.
        assert_eq!(
            t.hit("Rock_Stone", Some("Tool_Hatchet_Crude"), false, false),
            Hit::Unbreakable
        );
        // A weapon: no spec; a soft block still breaks.
        assert_eq!(
            t.hit("Rock_Stone", Some("Weapon_Sword"), false, false),
            Hit::Nothing
        );
        assert!(
            matches!(t.hit("Plant_Grass", Some("Weapon_Sword"), false, false), Hit::Damage { damage, .. } if damage == 1.0)
        );
        // Placed as decoration: the default drop.
        assert!(matches!(
            t.hit("Wood_Oak_Trunk", None, true, false),
            Hit::Damage { item: None, .. }
        ));
    }

    #[test]
    fn break_tools() {
        let t = Table::parse(T);
        assert_eq!(
            t.break_tool(Some("Tool_Shears"), "Primary"),
            (Some("Shears".into()), false)
        );
        assert_eq!(t.break_tool(None, "Primary"), (None, false));
        // A tool that must match does nothing on a block without its entry.
        assert_eq!(
            t.hit_with("Rock_Stone", None, false, false, Some("Scraper"), true),
            Hit::Nothing
        );
        match t.hit_with("Wood_Oak_Trunk", None, false, false, Some("Scraper"), true) {
            Hit::Damage { tool: Some(d), .. } => {
                assert_eq!(d.state.as_deref(), Some("Stripped"));
                assert_eq!(d.item.as_deref(), Some("Bark"));
            }
            h => panic!("{h:?}"),
        }
        assert!(matches!(
            t.hit_with("Plant_Grass", None, false, false, Some("Shears"), false),
            Hit::Damage { tool: Some(_), .. }
        ));
    }

    #[test]
    fn wear_and_broken_tools() {
        let t = Table::parse(T);
        let h = "Tool_Hatchet_Crude";
        // A broken tool keeps a quarter of its power.
        assert!(
            matches!(t.hit("Wood_Oak_Trunk", Some(h), false, true), Hit::Damage { damage, .. } if damage == 0.15 * 0.25)
        );
        assert_eq!(t.wear(h, "Wood_Oak_Trunk", |s| s == "Wood"), 0.25);
        // Not in a listed set: the item's own loss (0 here).
        assert_eq!(t.wear(h, "Wood_Oak_Trunk", |_| false), 0.0);
        // Soft blocks and items that are no tool wear nothing.
        assert_eq!(t.wear(h, "Plant_Grass", |_| true), 0.0);
        assert_eq!(t.wear("Weapon_Sword", "Rock_Stone", |_| true), 0.0);
    }
}
