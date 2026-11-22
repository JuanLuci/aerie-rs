//! Processing benches (spec 16): furnaces, campfires, the salvage bench
//! and the tannery. Each holds fuel, input and output slots, burns fuel
//! over time and turns inputs into outputs by the bench's timed recipes,
//! whether a window is open or not, as the official bench block does.

use std::collections::BTreeSet;

use crate::inventory::{Slot, Stack};

/// One input or fuel slot of the bench asset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotCfg {
    pub icon: Option<String>,
    pub resource: Option<String>,
    pub filter_valid: bool,
}

/// `ExtraOutput`: items made every so many fuel items burnt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extra {
    pub per_fuel: i32,
    pub outputs: Vec<(String, i32)>,
    /// Items not counted; the official check compares these ids with the
    /// fuel item's block key, so only block items can be ignored.
    pub ignored: Vec<String>,
}

/// A processing bench asset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// At most this much fuel time stored (-1: no limit).
    pub max_fuel: i32,
    pub fuel_drop: Option<String>,
    pub allow_no_input: bool,
    pub output_slots: i32,
    pub end_sound: i32,
    pub input: Vec<SlotCfg>,
    /// `None`: the bench needs no fuel.
    pub fuel: Option<Vec<SlotCfg>>,
    pub extra: Option<Extra>,
    /// Per tier from level 1: extra input slots, extra output slots, time
    /// reduction.
    pub tiers: Vec<(i32, i32, f32)>,
}

impl Config {
    /// The input slots at a tier (extra slots copy the first one).
    pub fn inputs(&self, tier: i32) -> Vec<SlotCfg> {
        match self.tier(tier) {
            Some((extra, _, _)) if !self.input.is_empty() => {
                vec![self.input[0].clone(); self.input.len() + extra.max(0) as usize]
            }
            _ => self.input.clone(),
        }
    }

    pub fn outputs(&self, tier: i32) -> i32 {
        self.output_slots + self.tier(tier).map_or(0, |t| t.1)
    }

    pub fn reduction(&self, tier: i32) -> f32 {
        self.tier(tier).map_or(0.0, |t| t.2)
    }

    fn tier(&self, tier: i32) -> Option<(i32, i32, f32)> {
        if self.tiers.is_empty() || tier < 1 || tier as usize > self.tiers.len() {
            return None;
        }
        Some(self.tiers[tier as usize - 1])
    }

    pub fn has_fuel(&self) -> bool {
        self.fuel.is_some()
    }
}

/// A material of a recipe (item, resource or tag) and how many.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub item: Option<String>,
    pub resource: Option<String>,
    pub tag: Option<i32>,
    pub quantity: i32,
}

/// A recipe the bench can run.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub inputs: Vec<Material>,
    /// (item, quantity) per output.
    pub outputs: Vec<(String, i32)>,
    pub seconds: f32,
}

/// What processing needs to know about items.
pub trait Items {
    fn max_stack(&self, item: &str) -> i32;
    /// Units of the material one item gives (`None`: not that material).
    fn units(&self, m: &Material, item: &str) -> Option<i32>;
    /// Units of a resource type one item gives.
    fn resource(&self, item: &str, resource: &str) -> Option<i32>;
    fn fuel_quality(&self, item: &str) -> f64;
    /// The block key an item places, if any.
    fn block_key(&self, item: &str) -> Option<String>;
    fn stack(&self, item: &str, n: i32) -> Stack;
}

/// The state of one bench block.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub input: Vec<Slot>,
    pub fuel: Vec<Slot>,
    pub output: Vec<Slot>,
    pub progress: f32,
    pub fuel_time: f32,
    pub last_fuel_total: i32,
    pub next_extra: i32,
    pub recipe: Option<String>,
    pub active: bool,
    /// The bench has ticked once (the official last tick game time).
    pub ticked: bool,
    pub processing_slots: BTreeSet<i32>,
    pub processing_fuel_slots: BTreeSet<i32>,
    /// Progress to show in the window, when the official bench sends it.
    pub progress_sent: Option<f32>,
    /// The bench's `FuelDropItemId` (set each tick).
    pub fuel_drop: Option<String>,
    /// Filled by [`State::all`]'s caller: the fuel left, as drop items.
    pub dropped_fuel: Vec<(String, i32)>,
}

/// What a tick asks the world to do.
#[derive(Debug, Default, PartialEq)]
pub struct Events {
    /// Switch the block to this interaction state (if it is not in it).
    pub state: Option<&'static str>,
    pub sounds: Vec<i32>,
    /// Items that did not fit in the output, thrown out of the front.
    pub eject: Vec<Stack>,
    /// The window's numbers changed.
    pub window: bool,
    /// Fuel to show (left of the last fuel, its total), when sent.
    pub fuel: Option<(f32, i32)>,
}

impl State {
    pub fn new(cfg: &Config, tier: i32) -> Self {
        let mut s = State {
            input: vec![None; cfg.inputs(tier).len()],
            fuel: vec![None; cfg.fuel.as_ref().map_or(0, Vec::len)],
            output: vec![None; cfg.outputs(tier).max(0) as usize],
            next_extra: cfg.extra.as_ref().map_or(0, |e| e.per_fuel),
            ..Default::default()
        };
        if !cfg.has_fuel() {
            s.active = true;
        }
        s
    }

    /// The window's slots: fuel, then input, then output.
    pub fn slots(&self) -> Vec<Slot> {
        self.fuel
            .iter()
            .chain(&self.input)
            .chain(&self.output)
            .cloned()
            .collect()
    }

    /// Puts the window's slots back.
    pub fn set_slots(&mut self, slots: Vec<Slot>) {
        let (f, i) = (self.fuel.len(), self.input.len());
        let mut it = slots.into_iter();
        self.fuel = it.by_ref().take(f).collect();
        self.input = it.by_ref().take(i).collect();
        self.output = it.collect();
    }

    /// Everything inside, for when the bench is broken; the whole seconds
    /// of fuel left become that many of the bench's fuel drop item (in
    /// `dropped_fuel`, split at 100 a stack by the caller's item rules).
    pub fn all(&mut self) -> Vec<Stack> {
        if let Some(item) = self.fuel_drop.clone() {
            let n = self.fuel_time as i32;
            if n > 0 {
                self.dropped_fuel.push((item, n));
            }
        }
        self.fuel_time = 0.0;
        self.slots().into_iter().flatten().collect()
    }

    /// `SetActiveAction` and the bench's own switching: on needs fuel
    /// (when the bench takes any). `true` if it changed.
    pub fn set_active(&mut self, on: bool, cfg: &Config) -> bool {
        if self.active == on {
            return false;
        }
        if on && cfg.has_fuel() && self.fuel.iter().all(Option::is_none) {
            return false;
        }
        self.active = on;
        if !on {
            self.processing_slots.clear();
            self.processing_fuel_slots.clear();
        }
        true
    }

    /// The fuel shown in the window: what is left of the last fuel burnt.
    pub fn fuel_shown(&mut self) -> (f32, i32) {
        if self.fuel_time > self.last_fuel_total as f32 {
            self.last_fuel_total = self.fuel_time.ceil() as i32;
        }
        let p = if self.last_fuel_total > 0 {
            self.fuel_time / self.last_fuel_total as f32
        } else {
            0.0
        };
        (p, self.last_fuel_total)
    }
}

/// A slot's leftover after taking `want` units of `m` from it (negative:
/// the items taken give more than wanted), as the official test.
fn test_slot(items: &impl Items, slot: &Slot, m: &Material, want: i32) -> i32 {
    let Some(s) = slot else { return want };
    let Some(u) = items.units(m, &s.item).filter(|&u| u > 0) else {
        return want;
    };
    let need = (want + u - 1) / u;
    let k = need.min(s.n);
    want - k * u
}

/// The slots each material would be taken from (empty if any material
/// is short or, in exact amounts, overshoots).
fn slots_to_take(items: &impl Items, slots: &[Slot], mats: &[Material]) -> Vec<Vec<i32>> {
    let mut out = Vec::new();
    for m in mats {
        let mut left = m.quantity;
        let mut picked = Vec::new();
        for (i, s) in slots.iter().enumerate() {
            if left <= 0 {
                break;
            }
            let n = test_slot(items, s, m, left);
            if n != left {
                picked.push(i as i32);
                left = n;
            }
        }
        if left != 0 {
            return Vec::new();
        }
        out.push(picked);
    }
    out
}

/// Takes the materials, all or nothing and in exact amounts.
fn take(items: &impl Items, slots: &mut [Slot], mats: &[Material]) -> bool {
    if slots_to_take(items, slots, mats).len() != mats.len() {
        return false;
    }
    for m in mats {
        let mut left = m.quantity;
        for s in slots.iter_mut() {
            if left <= 0 {
                break;
            }
            let Some(st) = s else { continue };
            let Some(u) = items.units(m, &st.item).filter(|&u| u > 0) else {
                continue;
            };
            let k = ((left + u - 1) / u).min(st.n);
            st.n -= k;
            left -= k * u;
            if st.n == 0 {
                *s = None;
            }
        }
    }
    true
}

/// Whether each stack would fit on its own (the official test, without
/// the slot filters).
fn can_add_each(items: &impl Items, slots: &[Slot], stacks: &[Stack]) -> bool {
    stacks.iter().all(|st| {
        let max = items.max_stack(&st.item).max(1);
        let mut left = st.n;
        for s in slots.iter().flatten() {
            if s.stacks_with(st) && s.n < max {
                left -= (max - s.n).min(left);
            }
        }
        for s in slots {
            if s.is_none() && left > 0 {
                left -= max.min(left);
            }
        }
        left <= 0
    })
}

/// Adds stacks (existing stacks first, then empty slots); what does not
/// fit comes back.
fn add_all(items: &impl Items, slots: &mut [Slot], stacks: Vec<Stack>) -> Vec<Stack> {
    let mut rest = Vec::new();
    for st in stacks {
        let max = items.max_stack(&st.item).max(1);
        let mut left = st.n;
        for s in slots.iter_mut().flatten() {
            if s.stacks_with(&st) && s.n < max && left > 0 {
                let k = (max - s.n).min(left);
                s.n += k;
                left -= k;
            }
        }
        for s in slots.iter_mut() {
            if s.is_none() && left > 0 {
                let k = max.min(left);
                *s = Some(st.with_n(k));
                left -= k;
            }
        }
        if left > 0 {
            rest.push(st.with_n(left));
        }
    }
    rest
}

/// Stacks of `outputs` × `count`, split at the max stack.
fn scaled(items: &impl Items, outputs: &[(String, i32)], count: i32) -> Vec<Stack> {
    let mut v = Vec::new();
    for (item, q) in outputs {
        let max = items.max_stack(item).max(1);
        let mut total = q * count;
        while total > 0 {
            let k = total.min(max);
            v.push(items.stack(item, k));
            total -= k;
        }
    }
    v
}

/// A bench at work: its asset, recipes and tier with the item facts.
pub struct Bench<'a, I: Items> {
    pub cfg: &'a Config,
    /// The bench's recipes (not restricted at its tier).
    pub recipes: &'a [Recipe],
    pub tier: i32,
    pub items: &'a I,
}

impl<I: Items> Bench<'_, I> {
    fn recipe<'r>(&'r self, s: &State) -> Option<&'r Recipe> {
        let id = s.recipe.as_ref()?;
        self.recipes.iter().find(|r| &r.id == id)
    }

    fn recipe_time(&self, r: &Recipe) -> f32 {
        let m = self.cfg.reduction(self.tier);
        if m > 0.0 {
            r.seconds - r.seconds * m
        } else {
            r.seconds
        }
    }

    /// Picks the recipe the inputs make (each material from its own slot;
    /// the one with the most materials, keeping the current one if still
    /// possible). A new recipe starts from no progress. `true` if the
    /// window's progress was reset.
    pub fn update_recipe(&self, s: &mut State) -> bool {
        let mut matching: Vec<&Recipe> = Vec::new();
        for r in self.recipes {
            let mut free: Vec<usize> = (0..s.input.len()).collect();
            let mut matches = 0;
            for m in &r.inputs {
                if let Some(k) = free
                    .iter()
                    .position(|&j| test_slot(self.items, &s.input[j], m, m.quantity) == 0)
                {
                    matches += 1;
                    free.remove(k);
                }
            }
            if matches == r.inputs.len() {
                matching.push(r);
            }
        }
        if matching.is_empty() {
            s.recipe = None;
            s.last_fuel_total = 0;
            s.progress = 0.0;
            s.progress_sent = Some(0.0);
            return true;
        }
        // Stable sort by input count, then reversed (as the official list).
        matching.sort_by_key(|r| r.inputs.len());
        matching.reverse();
        if let Some(cur) = &s.recipe
            && matching.iter().any(|r| &r.id == cur)
        {
            return false;
        }
        let first = matching[0];
        let reset = s.recipe.as_deref() != Some(&first.id);
        if reset {
            s.progress = 0.0;
            s.progress_sent = Some(0.0);
        }
        s.recipe = Some(first.id.clone());
        reset
    }

    fn available_sets(&self, s: &State, r: &Recipe) -> i32 {
        if r.inputs.is_empty() {
            return 0;
        }
        let mut min = i32::MAX;
        for m in &r.inputs {
            let have: i32 = s
                .input
                .iter()
                .flatten()
                .filter(|st| self.items.units(m, &st.item).is_some())
                .map(|st| st.n)
                .sum();
            min = min.min(have / m.quantity.max(1));
        }
        min
    }

    fn total_fuel(&self, s: &State) -> f32 {
        s.fuel
            .iter()
            .flatten()
            .map(|st| (st.n as f64 * self.items.fuel_quality(&st.item)) as f32)
            .sum()
    }

    /// Burns one fuel item; `None` when nothing could burn.
    fn burn_one(&self, s: &mut State, ev: &mut Events) -> Option<usize> {
        let slots = self.cfg.fuel.as_ref()?;
        if slots.is_empty()
            || !s.active
            || (self.cfg.max_fuel > 0 && s.fuel_time >= self.cfg.max_fuel as f32)
            || s.fuel.iter().all(Option::is_none)
        {
            return None;
        }
        if s.fuel_time < 0.0 {
            s.fuel_time = 0.0;
        }
        for (i, cfg) in slots.iter().enumerate() {
            let resource = cfg.resource.as_deref().unwrap_or("Fuel");
            let Some(st) = s.fuel.get(i).cloned().flatten() else {
                continue;
            };
            let quality = self.items.fuel_quality(&st.item);
            // One unit of the resource, from the first slot holding it, in
            // exact amounts.
            let Some((j, u)) = s.fuel.iter().enumerate().find_map(|(j, x)| {
                let x = x.as_ref()?;
                Some((j, self.items.resource(&x.item, resource)?))
            }) else {
                continue;
            };
            if u != 1 {
                continue;
            }
            let x = s.fuel[j].as_mut().unwrap();
            x.n -= 1;
            if x.n == 0 {
                s.fuel[j] = None;
            }
            if let Some(extra) = &self.cfg.extra {
                let ignored = self
                    .items
                    .block_key(&st.item)
                    .is_some_and(|b| extra.ignored.contains(&b));
                if !ignored {
                    s.next_extra -= 1;
                    if s.next_extra <= 0 {
                        s.next_extra = extra.per_fuel;
                        let stacks: Vec<Stack> = extra
                            .outputs
                            .iter()
                            .map(|(item, q)| self.items.stack(item, *q))
                            .collect();
                        ev.eject.extend(add_all(self.items, &mut s.output, stacks));
                    }
                }
            }
            s.fuel_time += (u as f64 * quality) as f32;
            return Some(i);
        }
        None
    }

    /// Fuel for `duration` seconds: the time left first, then whole fuel
    /// items.
    pub fn burn(&self, s: &mut State, duration: f32, ev: &mut Events) {
        if duration <= 0.0 {
            return;
        }
        let mut used = 0.0;
        if s.fuel_time > 0.0 {
            let u = s.fuel_time.min(duration);
            s.fuel_time -= u;
            used += u;
        }
        while used < duration && self.burn_one(s, ev).is_some() {
            let u = s.fuel_time.min(duration - used);
            s.fuel_time -= u;
            used += u;
        }
    }

    /// Advances by `dt` seconds (the official batch: as many crafts as
    /// time, inputs, fuel and output room allow). Returns crafts done.
    pub fn advance(&self, s: &mut State, dt: f32, ev: &mut Events) -> i32 {
        let Some(r) = self.recipe(s).cloned() else {
            return 0;
        };
        if dt <= 0.0 {
            return 0;
        }
        let fuel = self.cfg.has_fuel();
        if fuel && !s.active {
            return 0;
        }
        let time = self.recipe_time(&r);
        if time <= 0.0 {
            let mut done = 0;
            loop {
                let Some(r) = self.recipe(s).cloned() else {
                    break;
                };
                if s.progress < r.seconds {
                    break;
                }
                let outs = scaled(self.items, &r.outputs, 1);
                if !can_add_each(self.items, &s.output, &outs) {
                    break;
                }
                if !take(self.items, &mut s.input, &r.inputs) {
                    break;
                }
                s.progress -= r.seconds;
                ev.eject.extend(add_all(self.items, &mut s.output, outs));
                done += 1;
                self.update_recipe(s);
                if s.recipe.is_none() {
                    break;
                }
            }
            if fuel && s.active && s.recipe.is_none() {
                s.set_active(false, self.cfg);
            }
            return done;
        }
        let start = s.progress;
        let from_time = ((s.progress + dt) / time) as i32;
        let from_input = self.available_sets(s, &r);
        let from_fuel = if fuel {
            ((s.progress + s.fuel_time + self.total_fuel(s)) / time) as i32
        } else {
            i32::MAX
        };
        let mut n = from_time.min(from_input).min(from_fuel);
        if n > 0 {
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                if can_add_each(self.items, &s.output, &scaled(self.items, &r.outputs, mid)) {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            n = lo;
        }
        let for_crafts = if n > 0 {
            (n as f32 * time - s.progress).max(0.0)
        } else {
            0.0
        };
        let more = n < from_input;
        let mut partial = if more {
            (dt - for_crafts).min(time).max(0.0)
        } else {
            0.0
        };
        if fuel && partial > 0.0 {
            let total = s.fuel_time + self.total_fuel(s);
            partial = partial.min((total - for_crafts).max(0.0));
        }
        let used = for_crafts + partial;
        if fuel && used > 0.0 {
            self.burn(s, used, ev);
        }
        if n > 0 {
            let mats: Vec<Material> = r
                .inputs
                .iter()
                .map(|m| Material {
                    quantity: m.quantity * n,
                    ..m.clone()
                })
                .collect();
            if take(self.items, &mut s.input, &mats) {
                let outs = scaled(self.items, &r.outputs, n);
                ev.eject.extend(add_all(self.items, &mut s.output, outs));
            }
        }
        s.progress = (start + used - n as f32 * time).max(0.0);
        self.update_recipe(s);
        if fuel && s.active && s.recipe.is_none() {
            s.set_active(false, self.cfg);
        }
        n
    }

    /// One world tick of the bench (the official processing tick), given
    /// the block's current interaction state and the seconds since the
    /// last tick (0 for the first one).
    pub fn tick(&self, s: &mut State, current: &str, dt: f32, sounds: Sounds) -> Events {
        let mut ev = Events::default();
        let dt = if s.ticked { dt } else { 0.0 };
        s.ticked = true;
        let processing = current == "Processing";
        let completed = current == "ProcessCompleted";
        // Back to default: the failed sound from processing, the end sound
        // from completed.
        let reset = |ev: &mut Events| {
            if processing {
                ev.state = Some("default");
                ev.sounds.push(sounds.failed);
            } else if completed {
                ev.state = Some("default");
                ev.sounds.push(sounds.end);
            }
        };
        let before_slots = s.processing_slots.clone();
        s.processing_slots.clear();
        if s.recipe.is_some() && self.recipe(s).is_none() {
            self.update_recipe(s);
        }
        let fuel = self.cfg.has_fuel();
        let mut can = false;
        if let Some(r) = self.recipe(s).cloned() {
            let outs = scaled(self.items, &r.outputs, 1);
            if !can_add_each(self.items, &s.output, &outs) {
                reset(&mut ev);
                ev.window |= s.set_active(false, self.cfg);
                return ev;
            }
            let picked = slots_to_take(self.items, &s.input, &r.inputs);
            if picked.is_empty() {
                reset(&mut ev);
                s.progress = 0.0;
                ev.window |= s.set_active(false, self.cfg);
                s.recipe = None;
                return ev;
            }
            s.processing_slots.extend(picked.into_iter().flatten());
            can = true;
        } else {
            if !fuel {
                reset(&mut ev);
                return ev;
            }
            if !self.cfg.allow_no_input && processing {
                ev.state = Some("default");
                ev.sounds.push(sounds.failed);
            } else if completed {
                ev.state = Some("default");
                ev.sounds.push(sounds.end);
                ev.window |= s.set_active(false, self.cfg);
                s.progress_sent = Some(0.0);
                ev.window = true;
                return ev;
            }
            s.progress_sent = Some(0.0);
            ev.window = true;
            if !self.cfg.allow_no_input {
                ev.window |= s.set_active(false, self.cfg);
                return ev;
            }
        }
        if s.processing_slots != before_slots {
            ev.window = true;
        }
        let done = self.advance(s, dt, &mut ev);
        if !can && s.active {
            self.burn(s, dt, &mut ev);
        }
        s.processing_fuel_slots.clear();
        if fuel {
            if s.active && s.fuel_time > 0.0 {
                if let Some(i) = s.fuel.iter().position(Option::is_some) {
                    s.processing_fuel_slots.insert(i as i32);
                }
            }
            if !s.active || s.fuel_time <= 0.0 {
                s.last_fuel_total = 0;
                if processing || completed {
                    ev.state = Some("default");
                    ev.sounds.push(sounds.failed);
                    s.set_active(false, self.cfg);
                }
                ev.window = true;
                return ev;
            }
            ev.fuel = Some(s.fuel_shown());
            ev.window = true;
        }
        if done > 0 {
            ev.state = Some("ProcessCompleted");
            ev.sounds.push(sounds.completed);
        } else if !processing {
            ev.state = Some("Processing");
        }
        if done == 0 && can {
            s.progress_sent = Some(match self.recipe(s) {
                Some(r) if self.recipe_time(r) > 0.0 => s.progress / self.recipe_time(r),
                _ => 0.0,
            });
            ev.window = true;
        }
        ev
    }

    /// The window's progress (0 to 1) of the running craft.
    pub fn progress(&self, s: &State) -> f32 {
        match self.recipe(s) {
            Some(r) if self.recipe_time(r) > 0.0 => s.progress / self.recipe_time(r),
            _ => 0.0,
        }
    }

    /// Whether an item may go into input slot `slot` (the slot's resource,
    /// or any material of the bench's recipes).
    pub fn input_takes(&self, slot: usize, item: &str) -> bool {
        let inputs = self.cfg.inputs(self.tier);
        let Some(c) = inputs.get(slot) else {
            return false;
        };
        if let Some(r) = &c.resource {
            return self.items.resource(item, r).is_some();
        }
        if c.filter_valid {
            return self
                .recipes
                .iter()
                .flat_map(|r| &r.inputs)
                .any(|m| self.items.units(m, item).is_some());
        }
        true
    }

    /// Whether an item may go into fuel slot `slot`.
    pub fn fuel_takes(&self, slot: usize, item: &str) -> bool {
        match self.cfg.fuel.as_ref().and_then(|f| f.get(slot)) {
            Some(c) => c
                .resource
                .as_ref()
                .is_none_or(|r| self.items.resource(item, r).is_some()),
            None => false,
        }
    }
}

/// The bench's sounds (event indexes, 0 for none).
#[derive(Clone, Copy, Debug, Default)]
pub struct Sounds {
    pub failed: i32,
    pub end: i32,
    pub completed: i32,
}

/// A bitmask of slots (the window's `processingSlots`).
pub fn mask(slots: &BTreeSet<i32>) -> i32 {
    slots.iter().fold(0, |m, s| m | (1 << s))
}

/// Parses the bench lines of the benches table for one block.
pub fn parse(text: &str, block: &str) -> Option<Config> {
    let mut c: Option<Config> = None;
    let opt = |s: &str| (s != "-").then(|| s.to_owned());
    for l in text.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w[..] {
            ["P", k, max, drop, allow, outs, end] if k == block => {
                c = Some(Config {
                    max_fuel: max.parse().unwrap_or(-1),
                    fuel_drop: opt(drop),
                    allow_no_input: allow == "1",
                    output_slots: outs.parse().unwrap_or(1),
                    end_sound: end.parse().unwrap_or(0),
                    ..Default::default()
                });
            }
            ["PI", k, icon, res, filter] if k == block => {
                if let Some(c) = c.as_mut() {
                    c.input.push(SlotCfg {
                        icon: opt(icon),
                        resource: opt(res),
                        filter_valid: filter == "1",
                    });
                }
            }
            ["PF", k, icon, res] if k == block => {
                if let Some(c) = c.as_mut() {
                    c.fuel.get_or_insert_with(Vec::new).push(SlotCfg {
                        icon: opt(icon),
                        resource: opt(res),
                        filter_valid: false,
                    });
                }
            }
            ["PX", k, per, outs, ignored] if k == block => {
                if let Some(c) = c.as_mut() {
                    c.extra = Some(Extra {
                        per_fuel: per.parse().unwrap_or(1),
                        outputs: opt(outs)
                            .map(|o| {
                                o.split(',')
                                    .filter_map(|x| {
                                        let (i, q) = x.split_once(':')?;
                                        Some((i.to_owned(), q.parse().ok()?))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        ignored: opt(ignored)
                            .map(|o| o.split(',').map(str::to_owned).collect())
                            .unwrap_or_default(),
                    });
                }
            }
            ["PT", k, ei, eo, red] if k == block => {
                if let Some(c) = c.as_mut() {
                    c.tiers.push((
                        ei.parse().unwrap_or(0),
                        eo.parse().unwrap_or(0),
                        red.parse().unwrap_or(0.0),
                    ));
                }
            }
            _ => {}
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    struct T;
    impl Items for T {
        fn max_stack(&self, _: &str) -> i32 {
            100
        }
        fn units(&self, m: &Material, item: &str) -> Option<i32> {
            (m.item.as_deref() == Some(item)).then_some(1)
        }
        fn resource(&self, item: &str, r: &str) -> Option<i32> {
            (r == "Fuel" && item == "Log").then_some(1)
        }
        fn fuel_quality(&self, _: &str) -> f64 {
            4.0
        }
        fn block_key(&self, item: &str) -> Option<String> {
            Some(item.to_owned())
        }
        fn stack(&self, item: &str, n: i32) -> Stack {
            Stack::new(item, n, 0.0)
        }
    }

    fn furnace() -> Config {
        parse(
            "P Furnace -1 - 1 4 7\n\
             PI Furnace - - 1\n\
             PI Furnace - - 1\n\
             PF Furnace icon Fuel\n\
             PX Furnace 2 Charcoal:1 -\n",
            "Furnace",
        )
        .unwrap()
    }

    #[test]
    fn smelts_with_fuel() {
        let cfg = furnace();
        let recipes = vec![Recipe {
            id: "Bar".into(),
            inputs: vec![Material {
                item: Some("Ore".into()),
                resource: None,
                tag: None,
                quantity: 1,
            }],
            outputs: vec![("Bar".into(), 1)],
            seconds: 5.0,
        }];
        let b = Bench {
            cfg: &cfg,
            recipes: &recipes,
            tier: 1,
            items: &T,
        };
        let mut s = State::new(&cfg, 1);
        assert_eq!((s.input.len(), s.fuel.len(), s.output.len()), (2, 1, 4));
        assert!(!s.active);
        // No fuel: cannot switch on.
        assert!(!s.set_active(true, &cfg));
        s.fuel[0] = Some(Stack::new("Log", 3, 0.0));
        s.input[0] = Some(Stack::new("Ore", 3, 0.0));
        b.update_recipe(&mut s);
        assert_eq!(s.recipe.as_deref(), Some("Bar"));
        assert!(s.set_active(true, &cfg));
        let sounds = Sounds {
            failed: 1,
            end: 2,
            completed: 3,
        };
        // The first tick has no elapsed time: nothing burns yet.
        let ev = b.tick(&mut s, "default", 0.1, sounds);
        assert_eq!(ev.state, None);
        // 6 s: one bar, two logs burnt (4 s each), one charcoal.
        let ev = b.tick(&mut s, "Processing", 6.0, sounds);
        assert_eq!(ev.state, Some("ProcessCompleted"));
        // Fuel burns before the craft completes: the charcoal comes first.
        assert_eq!(s.output[0], Some(Stack::new("Charcoal", 1, 0.0)));
        assert_eq!(s.output[1], Some(Stack::new("Bar", 1, 0.0)));
        assert_eq!(s.input[0], Some(Stack::new("Ore", 2, 0.0)));
        assert_eq!(s.fuel[0], Some(Stack::new("Log", 1, 0.0)));
        assert!((s.progress - 1.0).abs() < 1e-4);
        assert_eq!(mask(&s.processing_slots), 1);
    }
}
