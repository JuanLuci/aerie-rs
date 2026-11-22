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

/// Most of one item in a slot when moving between slots.
const STACK: i32 = 999;

/// One slot's content: an item, how many, and its durability (the
/// official `ItemStack`: a new stack starts at the item's
/// `MaxDurability`; a max of 0 never wears).
#[derive(Clone, Debug, PartialEq)]
pub struct Stack {
    pub item: String,
    pub n: i32,
    pub durability: f64,
    pub max: f64,
}

impl Stack {
    /// A new stack at full durability.
    pub fn new(item: &str, n: i32, max: f64) -> Self {
        Self {
            item: item.to_owned(),
            n,
            durability: max,
            max,
        }
    }

    pub fn with_n(&self, n: i32) -> Self {
        Self { n, ..self.clone() }
    }

    pub fn unbreakable(&self) -> bool {
        self.max <= 0.0
    }

    pub fn broken(&self) -> bool {
        !self.unbreakable() && self.durability == 0.0
    }

    /// Durability changed by `by`, kept within 0 and the max.
    pub fn worn(&self, by: f64) -> Self {
        Self {
            durability: (self.durability + by).clamp(0.0, self.max.max(0.0)),
            ..self.clone()
        }
    }

    /// The official stacking rule: same item, durability and max.
    pub fn stacks_with(&self, o: &Stack) -> bool {
        self.item == o.item && self.durability == o.durability && self.max == o.max
    }

    pub fn packet(&self) -> proto::ItemWithAllMetadata {
        proto::ItemWithAllMetadata {
            item_id: self.item.clone(),
            quantity: self.n,
            durability: self.durability,
            max_durability: self.max,
            quality: -1,
            override_dropped_item_animation: false,
            metadata: None,
        }
    }
}

pub type Slot = Option<Stack>;

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
                *x = v.filter(|s| s.n > 0);
                true
            }
            None => false,
        }
    }

    /// Adds as the official pickup does (default preferences): stacks it
    /// stacks with over hotbar, storage, backpack in slot order, then empty
    /// slots in that order, each up to `max`. Returns what did not fit.
    pub fn add(&mut self, stack: &Stack, max: i32) -> i32 {
        let max = max.max(1);
        let mut n = stack.n;
        for sec in [HOTBAR, STORAGE, -9] {
            let Some(s) = self.section(sec) else { continue };
            for x in s.iter_mut().flatten() {
                if x.stacks_with(stack) && x.n < max && n > 0 {
                    let k = n.min(max - x.n);
                    x.n += k;
                    n -= k;
                }
            }
        }
        for sec in [HOTBAR, STORAGE, -9] {
            let Some(s) = self.section(sec) else { continue };
            for x in s.iter_mut() {
                if x.is_none() && n > 0 {
                    let k = n.min(max);
                    *x = Some(stack.with_n(k));
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
                let Some(st) = x else { continue };
                let Some(p) = per(&st.item).filter(|&p| p > 0) else {
                    continue;
                };
                let need = (units + p - 1) / p;
                let k = need.min(st.n);
                st.n -= k;
                units -= k * p;
                if st.n == 0 {
                    *x = None;
                }
            }
        }
        units.max(0)
    }

    /// Takes up to `n` from a slot.
    pub fn take(&mut self, section: i32, slot: i32, n: i32) -> Slot {
        let st = self.get(section, slot)?;
        let k = n.min(st.n).max(0);
        self.set(section, slot, Some(st.with_n(st.n - k)));
        (k > 0).then(|| st.with_n(k))
    }

    /// `MoveItemStack`: onto an empty slot or a stack it stacks with, else
    /// a swap (when the whole stack moves).
    pub fn move_stack(&mut self, from: (i32, i32), n: i32, to: (i32, i32)) {
        let Some(st) = self.get(from.0, from.1) else {
            return;
        };
        match self.get(to.0, to.1) {
            None => {
                let k = n.min(st.n);
                self.set(from.0, from.1, Some(st.with_n(st.n - k)));
                self.set(to.0, to.1, Some(st.with_n(k)));
            }
            Some(other) if other.stacks_with(&st) => {
                let k = n.min(st.n).min(STACK - other.n);
                self.set(from.0, from.1, Some(st.with_n(st.n - k)));
                self.set(to.0, to.1, Some(other.with_n(other.n + k)));
            }
            Some(other) if n >= st.n => {
                self.set(to.0, to.1, Some(st));
                self.set(from.0, from.1, Some(other));
            }
            Some(_) => {}
        }
    }

    /// `SmartMoveItemStack`: hotbar ↔ storage, into the first room.
    pub fn smart_move(&mut self, from: (i32, i32), n: i32) {
        let to = if from.0 == HOTBAR { STORAGE } else { HOTBAR };
        let Some(st) = self.take(from.0, from.1, n) else {
            return;
        };
        let mut m = st.n;
        if let Some(s) = self.section(to) {
            for x in s.iter_mut() {
                match x {
                    Some(o) if o.stacks_with(&st) && o.n < STACK && m > 0 => {
                        let k = m.min(STACK - o.n);
                        o.n += k;
                        m -= k;
                    }
                    None if m > 0 => {
                        *x = Some(st.with_n(m));
                        m = 0;
                    }
                    _ => {}
                }
            }
        }
        if m > 0 {
            // No room: back where it was.
            let back = self.get(from.0, from.1).map_or(0, |s| s.n) + m;
            self.set(from.0, from.1, Some(st.with_n(back)));
        }
    }

    /// Every stack, section by section.
    pub fn all(&self) -> Vec<(String, i32)> {
        self.sections
            .iter()
            .flat_map(|(_, v)| v.iter().flatten().map(|s| (s.item.clone(), s.n)))
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
                        .filter_map(|(i, s)| s.as_ref().map(|s| (i as i32, s.packet())))
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

    /// Text form for saving: `section slot item quantity durability max`
    /// per line.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for (id, v) in &self.sections {
            for (i, x) in v.iter().enumerate() {
                if let Some(st) = x {
                    s += &format!(
                        "{id} {i} {} {} {} {}\n",
                        st.item, st.n, st.durability, st.max
                    );
                }
            }
        }
        s
    }

    /// Reads [`Self::to_text`]; lines saved before durability was kept get
    /// the item's full durability from `max_of`.
    pub fn from_text(text: &str, max_of: impl Fn(&str) -> f64) -> Self {
        let mut inv = Self::default();
        for l in text.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            let (sec, slot, item, n) = match w[..] {
                [sec, slot, item, n, ..] => (sec, slot, item, n),
                _ => continue,
            };
            let (Ok(sec), Ok(slot), Ok(n)) = (sec.parse(), slot.parse(), n.parse()) else {
                continue;
            };
            let st = match w[..] {
                [_, _, _, _, d, m] => Stack {
                    item: item.to_owned(),
                    n,
                    durability: d.parse().unwrap_or(0.0),
                    max: m.parse().unwrap_or(0.0),
                },
                _ => Stack::new(item, n, max_of(item)),
            };
            inv.set(sec, slot, Some(st));
        }
        inv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(item: &str, n: i32) -> Slot {
        Some(Stack::new(item, n, 0.0))
    }

    #[test]
    fn add_move_swap_save() {
        let mut inv = Inventory::default();
        assert_eq!(inv.add(&Stack::new("Wood", 5, 0.0), 100), 0);
        assert_eq!(inv.get(HOTBAR, 0), s("Wood", 5));
        inv.add(&Stack::new("Stone", 3, 0.0), 100);
        inv.move_stack((HOTBAR, 0), 2, (STORAGE, 4));
        assert_eq!(inv.get(HOTBAR, 0), s("Wood", 3));
        assert_eq!(inv.get(STORAGE, 4), s("Wood", 2));
        // Whole stack onto another item: swap.
        inv.move_stack((HOTBAR, 1), 3, (HOTBAR, 0));
        assert_eq!(inv.get(HOTBAR, 0), s("Stone", 3));
        assert_eq!(inv.get(HOTBAR, 1), s("Wood", 3));
        inv.smart_move((HOTBAR, 0), 3);
        assert_eq!(inv.get(HOTBAR, 0), None);
        assert!(inv.to_text().contains("-2 0 Stone 3 0 0"));
        let back = Inventory::from_text(&inv.to_text(), |_| 9.0);
        assert_eq!(back.get(STORAGE, 0), s("Stone", 3));
        let old = Inventory::from_text("-1 0 Axe 1\n", |_| 9.0);
        assert_eq!(old.get(HOTBAR, 0), Some(Stack::new("Axe", 1, 9.0)));
        assert_eq!(inv.take(HOTBAR, 1, 10), s("Wood", 3));
        assert_eq!(inv.get(HOTBAR, 1), None);
    }

    #[test]
    fn worn_stacks_apart() {
        let mut inv = Inventory::default();
        let axe = Stack::new("Axe", 1, 200.0);
        inv.add(&axe, 1);
        let worn = axe.worn(-0.5);
        assert_eq!(worn.durability, 199.5);
        assert!(!worn.stacks_with(&axe));
        assert!(axe.worn(-500.0).broken());
        inv.add(&worn, 1);
        assert_eq!(inv.get(HOTBAR, 1), Some(worn));
    }
}
