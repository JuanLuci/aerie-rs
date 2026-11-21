//! Block damage and drops, from the gathering table built with the
//! registries (`aerie_assets::registries::gathering`).

use std::collections::HashMap;

/// Name of the table among the things built from the assets.
pub const TABLE: &str = "gathering";

#[derive(Clone, Debug, Default)]
pub struct Breaking {
    pub gather: Option<String>,
    pub quality: i32,
    pub item: Option<String>,
    pub quantity: i32,
    pub soft: bool,
    pub soft_item: Option<String>,
}

#[derive(Default)]
pub struct Table {
    /// Lower-cased block key → how it breaks.
    blocks: HashMap<String, Breaking>,
    /// Lower-cased item key → (gather type, power, quality).
    tools: HashMap<String, Vec<(String, f32, i32)>>,
    /// Gather type → bare-hand (power, quality).
    unarmed: HashMap<String, (f32, i32)>,
}

fn opt(s: &str) -> Option<String> {
    (s != "-").then(|| s.to_owned())
}

impl Table {
    pub fn parse(text: &str) -> Self {
        let mut t = Self::default();
        for l in text.lines() {
            let w: Vec<&str> = l.split(' ').collect();
            match w[..] {
                ["B", k, g, q, item, n, soft, soft_item] => {
                    t.blocks.insert(
                        k.to_lowercase(),
                        Breaking {
                            gather: opt(g),
                            quality: q.parse().unwrap_or(0),
                            item: opt(item),
                            quantity: n.parse().unwrap_or(1),
                            soft: soft == "1",
                            soft_item: opt(soft_item),
                        },
                    );
                }
                ["T", item, g, p, q] => t.tools.entry(item.to_lowercase()).or_default().push((
                    g.to_owned(),
                    p.parse().unwrap_or(0.0),
                    q.parse().unwrap_or(0),
                )),
                ["U", g, p, q] => {
                    t.unarmed.insert(
                        g.to_owned(),
                        (p.parse().unwrap_or(0.0), q.parse().unwrap_or(0)),
                    );
                }
                _ => {}
            }
        }
        t
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// The damage one hit does to `block` holding `held` (an item key), and
    /// what the block drops when it breaks; `None` when it cannot be broken
    /// that way.
    pub fn hit(&self, block: &str, held: Option<&str>) -> Option<(f32, Option<(String, i32)>)> {
        let b = self.blocks.get(&block.to_lowercase())?;
        if b.soft && b.gather.is_none() {
            return Some((1.0, b.soft_item.clone().map(|i| (i, 1))));
        }
        let g = b.gather.as_deref()?;
        let spec = match held.and_then(|h| self.tools.get(&h.to_lowercase())) {
            Some(specs) => specs.iter().find(|s| s.0 == g).map(|s| (s.1, s.2)),
            None => self.unarmed.get(g).copied(),
        };
        let (power, quality) = spec?;
        if quality < b.quality || power <= 0.0 {
            return None;
        }
        Some((power, b.item.clone().map(|i| (i, b.quantity.max(1)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "B Wood_Oak_Trunk Woods 0 Wood_Oak_Trunk 1 0 -\n\
                     B Rock_Stone Rocks 1 Rock_Stone_Cobble 1 0 -\n\
                     B Plant_Grass - 0 - 1 1 -\n\
                     T Tool_Hatchet_Crude Woods 0.15 0\n\
                     T Tool_Hatchet_Crude Rocks 0.05 0\n\
                     U Woods 0.05 0\n\
                     U Rocks 0.5 0\n";

    #[test]
    fn hands_tools_quality_and_drops() {
        let t = Table::parse(T);
        let (hand, drop) = t.hit("Wood_Oak_Trunk", None).unwrap();
        let (axe, _) = t.hit("wood_oak_trunk", Some("Tool_Hatchet_Crude")).unwrap();
        assert!(axe > hand, "an axe chops faster than a hand");
        assert_eq!(drop, Some(("Wood_Oak_Trunk".into(), 1)));
        // Rock needs quality 1: neither hands (0) nor this hatchet can.
        assert!(t.hit("Rock_Stone", None).is_none());
        assert_eq!(t.hit("Plant_Grass", None).unwrap().0, 1.0);
    }
}
