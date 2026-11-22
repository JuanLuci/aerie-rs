//! A player's inventory, section by section as the client knows it (the
//! official server's section ids and capacities): hotbar -1 (9), storage -2
//! (36), armor -3 (4), utility -5 (4), tools -8 (23), backpack -9 (0),
//! abilities -11 (6), rune bag -12 (50); an open window's own slots join as a
//! section with the window's id while a move runs (spec 14).

use aerie_protocol::proto;

pub const HOTBAR: i32 = -1;
pub const STORAGE: i32 = -2;
pub const ARMOR: i32 = -3;
pub const UTILITY: i32 = -5;
pub const BACKPACK: i32 = -9;
pub const ABILITIES: i32 = -11;
pub const RUNE_BAG: i32 = -12;
/// The official `EVERYTHING` order (merging stacks into a slot).
pub const EVERYTHING: [i32; 5] = [ARMOR, HOTBAR, UTILITY, STORAGE, BACKPACK];
/// Ability slots come in lines of a core and its supports.
const ABILITY_LINE: i32 = 3;

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

/// What the moves need to know about items (from the item registry).
pub trait Items {
    /// Most of the item in one slot (the official default 100).
    fn max_stack(&self, item: &str) -> i32;
    /// Its armor slot (0 head, 1 chest, 2 hands, 3 legs).
    fn armor_slot(&self, _item: &str) -> Option<i32> {
        None
    }
    /// A usable utility item.
    fn usable(&self, _item: &str) -> bool {
        false
    }
    fn ability(&self, _item: &str) -> Option<Ability> {
        None
    }
    /// Which pickup preference applies to it.
    fn kind(&self, _item: &str) -> Kind {
        Kind::Misc
    }
    /// The open window's input slot takes it.
    fn window_input(&self, _item: &str) -> bool {
        false
    }
}

/// An item's ability: a core, or a support with the keywords it needs of
/// its line's core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ability {
    pub core: bool,
    pub keywords: Vec<i32>,
    pub requires_any: Option<Vec<i32>>,
    pub requires_all: Option<Vec<i32>>,
    pub excludes_any: Option<Vec<i32>>,
}

impl Ability {
    /// Whether this support fits under `core` (the official keyword rule).
    fn supports(&self, core: &Ability) -> bool {
        let k = &core.keywords;
        let any = |v: &Vec<i32>| v.iter().any(|x| k.contains(x));
        if self.requires_any.as_ref().is_some_and(|v| !any(v)) {
            return false;
        }
        if self
            .requires_all
            .as_ref()
            .is_some_and(|v| !v.iter().all(|x| k.contains(x)))
        {
            return false;
        }
        self.excludes_any.as_ref().is_none_or(|v| !any(v))
    }
}

/// The kinds of items the pickup preferences tell apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Armor,
    WeaponOrTool,
    Usable,
    SolidBlock,
    Misc,
}

/// Where a kind of item goes first (`SyncPlayerPreferences`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Location {
    #[default]
    Hotbar,
    Storage,
    Backpack,
}

impl Location {
    pub fn from_wire(v: i32) -> Self {
        match v {
            1 => Location::Storage,
            2 => Location::Backpack,
            _ => Location::Hotbar,
        }
    }

    /// The sections, in order, as the official combined containers.
    pub fn order(self) -> [i32; 3] {
        match self {
            Location::Hotbar => [HOTBAR, STORAGE, BACKPACK],
            Location::Storage => [STORAGE, HOTBAR, BACKPACK],
            Location::Backpack => [BACKPACK, STORAGE, HOTBAR],
        }
    }
}

/// A player's pickup preferences (all hotbar by default).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Preferences {
    pub armor: Location,
    pub weapon_and_tool: Location,
    pub usable: Location,
    pub solid_block: Location,
    pub misc: Location,
}

impl Preferences {
    /// The sections an item goes to (the official pickup container).
    pub fn order_for(&self, kind: Kind) -> [i32; 3] {
        match kind {
            Kind::Armor => self.armor,
            Kind::WeaponOrTool => self.weapon_and_tool,
            Kind::Usable => self.usable,
            Kind::SolidBlock => self.solid_block,
            Kind::Misc => self.misc,
        }
        .order()
    }
}

/// `SmartMoveItemStack`'s kinds.
pub const EQUIP_OR_MERGE: i32 = 0;
pub const PUT_IN_HOTBAR_OR_WINDOW: i32 = 1;
pub const PUT_IN_HOTBAR_OR_BACKPACK: i32 = 2;

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
    /// While a move runs: the sections it set (sent even if unchanged, as
    /// the official server sends every successful transaction).
    touched: Option<Vec<i32>>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            sections: SECTIONS
                .iter()
                .map(|&(id, n)| (id, vec![None; n]))
                .collect(),
            touched: None,
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
        if let Some(t) = self.touched.as_mut() {
            t.push(section);
        }
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

    /// Joins an open window's slots as section `id` (structural benches:
    /// the input, then the options).
    pub fn attach(&mut self, id: i32, slots: Vec<Slot>) {
        self.sections.push((id, slots));
    }

    /// Takes the window's slots back out.
    pub fn detach(&mut self, id: i32) -> Option<Vec<Slot>> {
        let i = self.sections.iter().position(|(s, _)| *s == id)?;
        Some(self.sections.remove(i).1)
    }

    /// Starts noting the sections moves set.
    pub fn record(&mut self) {
        self.touched = Some(Vec::new());
    }

    /// The sections successful moves set since [`Self::record`].
    pub fn recorded(&mut self) -> Vec<i32> {
        self.touched.take().unwrap_or_default()
    }

    fn mark(&self) -> usize {
        self.touched.as_ref().map_or(0, Vec::len)
    }

    /// A failed move: what it set is not sent.
    fn unmark(&mut self, m: usize) {
        if let Some(t) = self.touched.as_mut() {
            t.truncate(m);
        }
    }

    fn capacity(&self, sec: i32) -> usize {
        self.sections
            .iter()
            .find(|(i, _)| *i == sec)
            .map_or(0, |(_, v)| v.len())
    }

    /// The official slot filters for adding `st` (spec 14).
    fn can_add(&self, items: &impl Items, sec: i32, slot: i32, st: &Stack) -> bool {
        match sec {
            ARMOR => items.armor_slot(&st.item) == Some(slot),
            UTILITY => items.usable(&st.item),
            RUNE_BAG => items.ability(&st.item).is_some(),
            ABILITIES => {
                let Some(a) = items.ability(&st.item) else {
                    return false;
                };
                let core_slot = slot - slot % ABILITY_LINE;
                let copy = |i: i32| self.get(ABILITIES, i).is_some_and(|s| s.item == st.item);
                if slot % ABILITY_LINE == 0 {
                    a.core
                        && !(0..self.capacity(ABILITIES) as i32)
                            .step_by(ABILITY_LINE as usize)
                            .any(|i| i != slot && copy(i))
                } else {
                    let core = self
                        .get(ABILITIES, core_slot)
                        .and_then(|c| items.ability(&c.item))
                        .filter(|c| c.core);
                    let end = (core_slot + ABILITY_LINE).min(self.capacity(ABILITIES) as i32);
                    !a.core
                        && core.is_some_and(|c| a.supports(&c))
                        && !(core_slot + 1..end).any(|i| i != slot && copy(i))
                }
            }
            s if s >= 0 => slot == 0 && items.window_input(&st.item),
            _ => true,
        }
    }

    /// A window's option slots give nothing (the official deny-all filter).
    fn can_remove(&self, sec: i32, slot: i32) -> bool {
        !(sec >= 0 && slot >= 1)
    }

    /// `MoveItemStack` as the official container does it: take `n` off the
    /// slot (all of it or nothing), then onto an empty slot, a stack it
    /// stacks with (up to the item's max, the rest back), or a swap when
    /// the whole stack left; filters can refuse. A slot moved onto itself
    /// ends as it was. `true` if something moved.
    pub fn move_to_slot(
        &mut self,
        items: &impl Items,
        from: (i32, i32),
        n: i32,
        to: (i32, i32),
    ) -> bool {
        let m = self.mark();
        let ok = self.move_to_slot_inner(items, from, n, to);
        if !ok {
            self.unmark(m);
        }
        ok
    }

    fn move_to_slot_inner(
        &mut self,
        items: &impl Items,
        from: (i32, i32),
        n: i32,
        to: (i32, i32),
    ) -> bool {
        if n < 0 || !self.can_remove(from.0, from.1) {
            return false;
        }
        let Some(before) = self.get(from.0, from.1) else {
            return false;
        };
        if usize::try_from(to.1).map_or(true, |i| i >= self.capacity(to.0)) {
            return false;
        }
        if n > before.n {
            return false;
        }
        if n == 0 {
            return true;
        }
        let moved = before.with_n(n);
        self.set(from.0, from.1, Some(before.with_n(before.n - n)));
        let left = self.get(from.0, from.1);
        let there = self.get(to.0, to.1);
        if !self.can_add(items, to.0, to.1, &moved) {
            self.set(from.0, from.1, Some(before));
            return false;
        }
        let Some(there) = there else {
            self.set(to.0, to.1, Some(moved));
            return true;
        };
        if !moved.stacks_with(&there) {
            if left.is_none() && self.can_add(items, from.0, from.1, &there) {
                self.set(from.0, from.1, Some(there));
                self.set(to.0, to.1, Some(moved));
                return true;
            }
            self.set(from.0, from.1, Some(before));
            return false;
        }
        let max = items.max_stack(&moved.item);
        let total = moved.n + there.n;
        if total <= max {
            self.set(to.0, to.1, Some(there.with_n(total)));
        } else {
            self.set(to.0, to.1, Some(there.with_n(max)));
            let rest = total - max + left.map_or(0, |l| l.n);
            self.set(from.0, from.1, Some(moved.with_n(rest)));
        }
        true
    }

    /// Adds into the sections in order as the official combined container:
    /// stacks it stacks with first, then empty slots, each up to the item's
    /// max and through the filters. Returns what did not fit.
    pub fn add_to(&mut self, items: &impl Items, order: &[i32], st: &Stack) -> i32 {
        let max = items.max_stack(&st.item).max(1);
        let mut n = st.n;
        for empty in [false, true] {
            for &sec in order {
                for i in 0..self.capacity(sec) as i32 {
                    if n <= 0 {
                        return 0;
                    }
                    let here = self.get(sec, i);
                    let fits = match &here {
                        None => empty,
                        Some(h) => !empty && h.stacks_with(st) && h.n < max,
                    };
                    if !fits || !self.can_add(items, sec, i, st) {
                        continue;
                    }
                    let have = here.as_ref().map_or(0, |h| h.n);
                    let k = n.min(max - have);
                    self.set(sec, i, Some(st.with_n(have + k)));
                    n -= k;
                }
            }
        }
        n
    }

    /// Takes `n` off a slot (all or nothing) and adds it into `order`; what
    /// does not fit goes back. `None` if nothing could be taken, else what
    /// did not fit.
    pub fn move_into(
        &mut self,
        items: &impl Items,
        from: (i32, i32),
        n: i32,
        order: &[i32],
    ) -> Option<i32> {
        if n < 0 || !self.can_remove(from.0, from.1) {
            return None;
        }
        let before = self.get(from.0, from.1)?;
        if n > before.n || n == 0 {
            return None;
        }
        let m = self.mark();
        let moved = before.with_n(n);
        self.set(from.0, from.1, Some(before.with_n(before.n - n)));
        let rest = self.add_to(items, order, &moved);
        if rest == n {
            self.unmark(m);
        }
        if rest > 0 {
            let back = self.get(from.0, from.1).map_or(0, |s| s.n) + rest;
            self.set(from.0, from.1, Some(moved.with_n(back)));
        }
        Some(rest)
    }

    /// Gathers stacks of the target's kind from `sources` into it (the
    /// official double click): the smallest stacks first (then by place),
    /// until the target is full.
    pub fn combine_into(&mut self, items: &impl Items, sources: &[i32], target: (i32, i32)) {
        let Some(t) = self.get(target.0, target.1) else {
            return;
        };
        let max = items.max_stack(&t.item);
        if t.n >= max {
            return;
        }
        let mut found: Vec<(i32, usize, (i32, i32))> = Vec::new();
        let mut k = 0;
        for &sec in sources {
            for i in 0..self.capacity(sec) as i32 {
                if self.can_remove(sec, i)
                    && (sec, i) != target
                    && let Some(s) = self.get(sec, i)
                    && s.stacks_with(&t)
                {
                    found.push((s.n, k, (sec, i)));
                }
                k += 1;
            }
        }
        found.sort();
        let mut have = t.n;
        for (_, _, at) in found {
            if have >= max {
                break;
            }
            let n = self.get(at.0, at.1).map_or(0, |s| s.n);
            self.move_to_slot(items, at, n, target);
            have = self.get(target.0, target.1).map_or(0, |s| s.n);
        }
    }

    /// An armor piece onto its armor slot (if empty, unless `force`).
    /// `true` when the item is armor that may go there (moved or not).
    fn try_equip(&mut self, items: &impl Items, from: (i32, i32), n: i32, force: bool) -> bool {
        let Some(st) = self.get(from.0, from.1) else {
            return false;
        };
        let Some(a) = items.armor_slot(&st.item) else {
            return false;
        };
        if from.0 == ARMOR || (!force && self.get(ARMOR, a).is_some()) {
            return false;
        }
        self.move_to_slot(items, from, n, (ARMOR, a));
        true
    }

    /// `SmartMoveItemStack` as the official server (spec 14): merge stacks
    /// into the slot (double click), put it in the hotbar or the open
    /// window (shift click), or in the hotbar or backpack. `window`: the
    /// open window's section, if it has slots.
    pub fn smart_move(
        &mut self,
        items: &impl Items,
        prefs: &Preferences,
        window: Option<i32>,
        from: (i32, i32),
        n: i32,
        kind: i32,
    ) {
        let Some(st) = self.get(from.0, from.1) else {
            return;
        };
        let pickup = prefs.order_for(items.kind(&st.item));
        match kind {
            EQUIP_OR_MERGE => {
                if self.try_equip(items, from, n, true) {
                    return;
                }
                if let Some(w) = window {
                    self.combine_into(items, &[w], from);
                }
                self.combine_into(items, &EVERYTHING, from);
            }
            PUT_IN_HOTBAR_OR_WINDOW => {
                if from.0 >= 0 {
                    self.move_into(items, from, n, &pickup);
                    return;
                }
                if let Some(w) = window {
                    match self.move_into(items, from, n, &[w]) {
                        Some(rest) if rest == n => {}
                        _ => return,
                    }
                }
                if self.try_equip(items, from, n, false) {
                    return;
                }
                let to: &[i32] = match from.0 {
                    STORAGE => &[HOTBAR],
                    HOTBAR => &[STORAGE],
                    _ => &pickup,
                };
                self.move_into(items, from, n, to);
            }
            PUT_IN_HOTBAR_OR_BACKPACK => {
                if from.0 == BACKPACK {
                    self.move_into(items, from, n, &pickup);
                } else {
                    self.move_into(items, from, n, &Location::Backpack.order());
                }
            }
            _ => {}
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

    struct T;
    impl Items for T {
        fn max_stack(&self, item: &str) -> i32 {
            if item == "Axe" { 1 } else { 100 }
        }
        fn armor_slot(&self, item: &str) -> Option<i32> {
            (item == "Helmet").then_some(0)
        }
    }

    fn s(item: &str, n: i32) -> Slot {
        Some(Stack::new(item, n, 0.0))
    }

    #[test]
    fn moves_as_the_official_container() {
        let mut inv = Inventory::default();
        assert_eq!(inv.add(&Stack::new("Wood", 5, 0.0), 100), 0);
        inv.add(&Stack::new("Stone", 3, 0.0), 100);
        assert!(inv.move_to_slot(&T, (HOTBAR, 0), 2, (STORAGE, 4)));
        assert_eq!(inv.get(HOTBAR, 0), s("Wood", 3));
        assert_eq!(inv.get(STORAGE, 4), s("Wood", 2));
        // Onto itself: unchanged, whole or in part (the double click's
        // first packet).
        assert!(inv.move_to_slot(&T, (HOTBAR, 0), 3, (HOTBAR, 0)));
        assert!(inv.move_to_slot(&T, (HOTBAR, 0), 1, (HOTBAR, 0)));
        assert_eq!(inv.get(HOTBAR, 0), s("Wood", 3));
        // More than the slot has: nothing.
        assert!(!inv.move_to_slot(&T, (HOTBAR, 0), 4, (HOTBAR, 5)));
        // Whole stack onto another item: swap; part of it: refused.
        assert!(!inv.move_to_slot(&T, (HOTBAR, 1), 2, (HOTBAR, 0)));
        assert!(inv.move_to_slot(&T, (HOTBAR, 1), 3, (HOTBAR, 0)));
        assert_eq!(inv.get(HOTBAR, 0), s("Stone", 3));
        assert_eq!(inv.get(HOTBAR, 1), s("Wood", 3));
        // Up to the max, the rest stays.
        inv.set(HOTBAR, 2, s("Wood", 99));
        assert!(inv.move_to_slot(&T, (HOTBAR, 1), 3, (HOTBAR, 2)));
        assert_eq!(inv.get(HOTBAR, 2), s("Wood", 100));
        assert_eq!(inv.get(HOTBAR, 1), s("Wood", 2));
        // Armor only where it fits.
        inv.set(HOTBAR, 3, s("Helmet", 1));
        assert!(!inv.move_to_slot(&T, (HOTBAR, 1), 2, (ARMOR, 0)));
        assert!(!inv.move_to_slot(&T, (HOTBAR, 3), 1, (ARMOR, 1)));
        assert!(inv.move_to_slot(&T, (HOTBAR, 3), 1, (ARMOR, 0)));
    }

    #[test]
    fn double_click_merges_never_duplicates() {
        let mut inv = Inventory::default();
        inv.set(HOTBAR, 4, s("Wood", 10));
        inv.set(STORAGE, 0, s("Wood", 7));
        inv.set(STORAGE, 1, s("Wood", 2));
        inv.set(STORAGE, 2, s("Stone", 5));
        let total =
            |i: &Inventory| -> i32 { i.all().iter().filter(|x| x.0 == "Wood").map(|x| x.1).sum() };
        for _ in 0..5 {
            inv.move_to_slot(&T, (HOTBAR, 4), 10, (HOTBAR, 4));
            inv.smart_move(
                &T,
                &Preferences::default(),
                None,
                (HOTBAR, 4),
                10,
                EQUIP_OR_MERGE,
            );
        }
        assert_eq!(total(&inv), 19);
        assert_eq!(inv.get(HOTBAR, 4), s("Wood", 19));
        assert_eq!(inv.get(STORAGE, 2), s("Stone", 5));
    }

    #[test]
    fn shift_click_and_backpack() {
        let mut inv = Inventory::default();
        inv.set(HOTBAR, 0, s("Wood", 5));
        let p = Preferences::default();
        inv.smart_move(&T, &p, None, (HOTBAR, 0), 5, PUT_IN_HOTBAR_OR_WINDOW);
        assert_eq!(inv.get(STORAGE, 0), s("Wood", 5));
        inv.smart_move(&T, &p, None, (STORAGE, 0), 5, PUT_IN_HOTBAR_OR_WINDOW);
        assert_eq!(inv.get(HOTBAR, 0), s("Wood", 5));
        // A helmet from the hotbar goes on.
        inv.set(HOTBAR, 1, s("Helmet", 1));
        inv.smart_move(&T, &p, None, (HOTBAR, 1), 1, PUT_IN_HOTBAR_OR_WINDOW);
        assert_eq!(inv.get(ARMOR, 0), s("Helmet", 1));
    }

    #[test]
    fn save_and_take() {
        let mut inv = Inventory::default();
        inv.set(STORAGE, 0, s("Stone", 3));
        inv.set(HOTBAR, 1, s("Wood", 3));
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
