//! A player's inventory, section by section as the client knows it (the
//! official server's section ids and capacities): hotbar -1 (9), storage -2
//! (36), armor -3 (4), utility -5 (4), tools -8 (23), backpack -9 (0),
//! abilities -11 (6), rune bag -12 (50).

use aerie_protocol::proto;

pub const HOTBAR: i32 = -1;
pub const STORAGE: i32 = -2;

const SECTIONS: [(i32, usize); 8] = [
    (-1, 9),
    (-2, 36),
    (-3, 4),
    (-5, 4),
    (-8, 23),
    (-9, 0),
    (-11, 6),
    (-12, 50),
];

/// Most of one item in a slot.
const STACK: i32 = 999;

pub type Slot = Option<(String, i32)>;

#[derive(Clone, Debug)]
pub struct Inventory {
    sections: Vec<(i32, Vec<Slot>)>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            sections: SECTIONS
                .iter()
                .map(|&(id, n)| (id, vec![None; n]))
                .collect(),
        }
    }
}

impl Inventory {
    fn section(&mut self, id: i32) -> Option<&mut Vec<Slot>> {
        self.sections
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, v)| v)
    }

    pub fn get(&self, section: i32, slot: i32) -> Slot {
        let s = self.sections.iter().find(|(i, _)| *i == section)?;
        s.1.get(usize::try_from(slot).ok()?).cloned().flatten()
    }

    pub fn set(&mut self, section: i32, slot: i32, v: Slot) -> bool {
        let Some(s) = self.section(section) else {
            return false;
        };
        match usize::try_from(slot).ok().and_then(|i| s.get_mut(i)) {
            Some(x) => {
                *x = v.filter(|(_, n)| *n > 0);
                true
            }
            None => false,
        }
    }

    /// Adds as the official pickup does (default preferences): existing
    /// stacks of the item over hotbar, storage, backpack in slot order, then
    /// empty slots in that order, each up to `max`. Returns what did not fit.
    pub fn add(&mut self, item: &str, mut n: i32, max: i32) -> i32 {
        let max = max.max(1);
        for sec in [HOTBAR, STORAGE, -9] {
            let Some(s) = self.section(sec) else { continue };
            for x in s.iter_mut().flatten() {
                if x.0 == item && x.1 < max && n > 0 {
                    let k = n.min(max - x.1);
                    x.1 += k;
                    n -= k;
                }
            }
        }
        for sec in [HOTBAR, STORAGE, -9] {
            let Some(s) = self.section(sec) else { continue };
            for x in s.iter_mut() {
                if x.is_none() && n > 0 {
                    let k = n.min(max);
                    *x = Some((item.to_owned(), k));
                    n -= k;
                }
            }
        }
        n
    }

    /// Takes `units` of a material from backpack, storage then hotbar (the
    /// official crafting order). `per(item)` is how many units one item
    /// gives, `None` when it is not the material. Returns the units still
    /// missing; slots are emptied as they are used.
    pub fn take_units(&mut self, per: impl Fn(&str) -> Option<i32>, mut units: i32) -> i32 {
        for sec in [-9, STORAGE, HOTBAR] {
            let Some(s) = self.section(sec) else { continue };
            for x in s.iter_mut() {
                if units <= 0 {
                    return 0;
                }
                let Some((item, have)) = x else { continue };
                let Some(p) = per(item).filter(|&p| p > 0) else {
                    continue;
                };
                let need = (units + p - 1) / p;
                let k = need.min(*have);
                *have -= k;
                units -= k * p;
                if *have == 0 {
                    *x = None;
                }
            }
        }
        units.max(0)
    }

    /// Takes up to `n` from a slot.
    pub fn take(&mut self, section: i32, slot: i32, n: i32) -> Slot {
        let (item, have) = self.get(section, slot)?;
        let k = n.min(have).max(0);
        self.set(section, slot, Some((item.clone(), have - k)));
        (k > 0).then_some((item, k))
    }

    /// `MoveItemStack`: onto an empty slot or the same item, else a swap
    /// (when the whole stack moves).
    pub fn move_stack(&mut self, from: (i32, i32), n: i32, to: (i32, i32)) {
        let Some((item, have)) = self.get(from.0, from.1) else {
            return;
        };
        match self.get(to.0, to.1) {
            None => {
                let k = n.min(have);
                self.set(from.0, from.1, Some((item.clone(), have - k)));
                self.set(to.0, to.1, Some((item, k)));
            }
            Some((other, m)) if other == item => {
                let k = n.min(have).min(STACK - m);
                self.set(from.0, from.1, Some((item.clone(), have - k)));
                self.set(to.0, to.1, Some((item, m + k)));
            }
            Some(other) if n >= have => {
                self.set(to.0, to.1, Some((item, have)));
                self.set(from.0, from.1, Some(other));
            }
            Some(_) => {}
        }
    }

    /// `SmartMoveItemStack`: hotbar ↔ storage, into the first room.
    pub fn smart_move(&mut self, from: (i32, i32), n: i32) {
        let to = if from.0 == HOTBAR { STORAGE } else { HOTBAR };
        let Some((item, _)) = self.take(from.0, from.1, n) else {
            return;
        };
        let k = {
            let total = self.get(from.0, from.1).map_or(0, |s| s.1);
            let _ = total;
            let mut left = 0;
            if let Some(s) = self.section(to) {
                let mut m = n;
                for x in s.iter_mut() {
                    match x {
                        Some((i, c)) if *i == item && *c < STACK && m > 0 => {
                            let k = m.min(STACK - *c);
                            *c += k;
                            m -= k;
                        }
                        None if m > 0 => {
                            *x = Some((item.clone(), m));
                            m = 0;
                        }
                        _ => {}
                    }
                }
                left = m;
            }
            left
        };
        if k > 0 {
            // No room: back where it was.
            let back = self.get(from.0, from.1).map_or(0, |s| s.1) + k;
            self.set(from.0, from.1, Some((item, back)));
        }
    }

    /// Every stack, section by section.
    pub fn all(&self) -> Vec<(String, i32)> {
        self.sections
            .iter()
            .flat_map(|(_, v)| v.iter().flatten().cloned())
            .collect()
    }

    /// Sections whose content differs from `before`'s.
    pub fn changed_since(&self, before: &Inventory) -> Vec<i32> {
        self.sections
            .iter()
            .zip(&before.sections)
            .filter(|(a, b)| a.1 != b.1)
            .map(|(a, _)| a.0)
            .collect()
    }

    /// The whole inventory as `UpdatePlayerInventory`.
    pub fn packet(&self) -> proto::UpdatePlayerInventory {
        let all: Vec<i32> = self.sections.iter().map(|(i, _)| *i).collect();
        self.packet_of(&all)
    }

    /// `UpdatePlayerInventory` with only the given sections (the others
    /// null), as the official server sends changes once per tick.
    pub fn packet_of(&self, which: &[i32]) -> proto::UpdatePlayerInventory {
        let sec = |id: i32| {
            if !which.contains(&id) {
                return None;
            }
            let (_, v) = self.sections.iter().find(|(i, _)| *i == id).unwrap();
            Some(proto::InventorySection {
                items: Some(
                    v.iter()
                        .enumerate()
                        .filter_map(|(i, s)| {
                            s.as_ref().map(|(item, n)| {
                                (
                                    i as i32,
                                    proto::ItemWithAllMetadata {
                                        item_id: item.clone(),
                                        quantity: *n,
                                        durability: 0.0,
                                        max_durability: 0.0,
                                        quality: -1,
                                        override_dropped_item_animation: false,
                                        metadata: None,
                                    },
                                )
                            })
                        })
                        .collect(),
                ),
                capacity: v.len() as i16,
            })
        };
        proto::UpdatePlayerInventory {
            storage: sec(-2),
            armor: sec(-3),
            hotbar: sec(-1),
            utility: sec(-5),
            tools: sec(-8),
            backpack: sec(-9),
            ability_slots: sec(-11),
            rune_bag: sec(-12),
        }
    }

    /// Text form for saving: `section slot item quantity` per line.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for (id, v) in &self.sections {
            for (i, x) in v.iter().enumerate() {
                if let Some((item, n)) = x {
                    s += &format!("{id} {i} {item} {n}\n");
                }
            }
        }
        s
    }

    pub fn from_text(text: &str) -> Self {
        let mut inv = Self::default();
        for l in text.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            if let [sec, slot, item, n] = w[..]
                && let (Ok(sec), Ok(slot), Ok(n)) = (sec.parse(), slot.parse(), n.parse())
            {
                inv.set(sec, slot, Some((item.to_owned(), n)));
            }
        }
        inv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_move_swap_save() {
        let mut inv = Inventory::default();
        assert_eq!(inv.add("Wood", 5, 100), 0);
        assert_eq!(inv.get(HOTBAR, 0), Some(("Wood".into(), 5)));
        inv.add("Stone", 3, 100);
        inv.move_stack((HOTBAR, 0), 2, (STORAGE, 4));
        assert_eq!(inv.get(HOTBAR, 0), Some(("Wood".into(), 3)));
        assert_eq!(inv.get(STORAGE, 4), Some(("Wood".into(), 2)));
        // Whole stack onto another item: swap.
        inv.move_stack((HOTBAR, 1), 3, (HOTBAR, 0));
        assert_eq!(inv.get(HOTBAR, 0), Some(("Stone".into(), 3)));
        assert_eq!(inv.get(HOTBAR, 1), Some(("Wood".into(), 3)));
        inv.smart_move((HOTBAR, 0), 3);
        assert_eq!(inv.get(HOTBAR, 0), None);
        assert!(inv.to_text().contains("-2 0 Stone 3"));
        let back = Inventory::from_text(&inv.to_text());
        assert_eq!(back.get(STORAGE, 0), Some(("Stone".into(), 3)));
        assert_eq!(inv.take(HOTBAR, 1, 10), Some(("Wood".into(), 3)));
        assert_eq!(inv.get(HOTBAR, 1), None);
    }
}
