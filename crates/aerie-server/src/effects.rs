//! Entity effects (spec 19): what is active on an entity, how effects
//! overlap, their periodic stat changes, and the updates viewers get.

use aerie_protocol::proto::{self as p, OverlapBehavior as Overlap};
use aerie_protocol::wire::Wire;

use crate::stats::Cond;

/// The name of the effects table among the registry extras.
pub const TABLE: &str = "entity-effects";

/// What removing an effect does.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Removal {
    /// The effect goes.
    #[default]
    Complete,
    /// It stops being infinite and runs out its duration.
    Infinite,
    /// Its duration ends: it goes at the next tick.
    Duration,
}

#[derive(Clone, Debug, Default)]
pub struct Effect {
    pub duration: f32,
    pub infinite: bool,
    pub debuff: bool,
    pub icon: Option<String>,
    pub overlap: Overlap,
    /// Seconds between stat changes; 0: once.
    pub cooldown: f32,
    /// Stat changes of a cycle: (stat, amount).
    pub stats: Vec<(i32, f32)>,
    pub percent: bool,
    pub removal: Removal,
    pub invulnerable: bool,
    /// Modifiers on stat maximums while active: (stat, multiplicative,
    /// amount).
    pub modifiers: Vec<(i32, bool, f32)>,
    /// The entity must meet these for the effect to apply and to stay.
    pub conditions: Vec<Cond>,
}

#[derive(Default)]
pub struct Book {
    pub effects: Vec<Option<Effect>>,
}

impl Book {
    /// From the effects registry packet and the server table (one line
    /// per effect: `index<TAB>{RemovalBehavior, Invulnerable,
    /// RawStatModifiers: [[stat, multiplicative, amount]], ApplyConditions}`).
    pub fn build(registry: &[u8], table: &str) -> anyhow::Result<Book> {
        let (m, _) = p::UpdateEntityEffects::decode(registry, 0)?;
        let mut book = Book::default();
        for (i, e) in m.entity_effects.into_iter().flatten() {
            let Ok(i) = usize::try_from(i) else { continue };
            if book.effects.len() <= i {
                book.effects.resize_with(i + 1, || None);
            }
            book.effects[i] = Some(Effect {
                duration: e.duration,
                infinite: e.infinite,
                debuff: e.debuff,
                icon: e.status_effect_icon,
                overlap: e.overlap_behavior,
                cooldown: e.damage_calculator_cooldown as f32,
                stats: e.stat_modifiers.unwrap_or_default(),
                percent: e.value_type == p::ValueType::PERCENT,
                ..Default::default()
            });
        }
        for l in table.lines() {
            let Some((i, json)) = l.split_once('\t') else {
                continue;
            };
            let (Ok(i), Ok(v)) = (
                i.parse::<usize>(),
                serde_json::from_str::<serde_json::Value>(json),
            ) else {
                continue;
            };
            let Some(Some(e)) = book.effects.get_mut(i) else {
                continue;
            };
            e.removal = match v.get("RemovalBehavior").and_then(|x| x.as_str()) {
                Some(s) if s.eq_ignore_ascii_case("Infinite") => Removal::Infinite,
                Some(s) if s.eq_ignore_ascii_case("Duration") => Removal::Duration,
                _ => Removal::Complete,
            };
            e.invulnerable = v.get("Invulnerable").and_then(|x| x.as_bool()) == Some(true);
            for m in v
                .get("RawStatModifiers")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                if let (Some(stat), Some(mult), Some(amount)) = (
                    m.get(0).and_then(|x| x.as_i64()),
                    m.get(1).and_then(|x| x.as_bool()),
                    m.get(2).and_then(|x| x.as_f64()),
                ) {
                    e.modifiers.push((stat as i32, mult, amount as f32));
                }
            }
            e.conditions = crate::stats::conds(v.get("ApplyConditions"));
        }
        Ok(book)
    }

    pub fn get(&self, i: i32) -> Option<&Effect> {
        self.effects.get(usize::try_from(i).ok()?)?.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct Active {
    pub index: i32,
    pub remaining: f32,
    pub infinite: bool,
    debuff: bool,
    icon: Option<String>,
    since_cycle: f32,
    cycled: bool,
    invulnerable: bool,
}

impl Active {
    fn update(&self, infinite: bool) -> p::EntityEffectUpdate {
        p::EntityEffectUpdate {
            r#type: p::EffectOp::ADD,
            id: self.index,
            remaining_time: self.remaining,
            infinite,
            debuff: self.debuff,
            status_effect_icon: self.icon.clone(),
        }
    }
}

/// The effects active on one entity.
#[derive(Default)]
pub struct Controller {
    active: Vec<Active>,
    changes: Vec<p::EntityEffectUpdate>,
    /// An active effect makes the entity invulnerable.
    pub invulnerable: bool,
}

impl Controller {
    pub fn has(&self, index: i32) -> bool {
        self.active.iter().any(|a| a.index == index)
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// The active effects in the order the official server lists them.
    fn order(&self) -> Vec<i32> {
        crate::changes::map_order(self.active.iter().map(|a| a.index))
    }

    /// Adds an effect, for `duration` seconds or the effect's own way
    /// (infinite or its duration). `ok`: its conditions hold. True when a
    /// new effect became active (stat modifiers must be recomputed).
    pub fn add(&mut self, book: &Book, index: i32, duration: Option<f32>, ok: bool) -> bool {
        let Some(e) = book.get(index) else {
            return false;
        };
        if !ok {
            return false;
        }
        let at = self.active.iter().position(|a| a.index == index);
        if duration.is_none() && e.infinite {
            let new = at.is_none();
            let i = at.unwrap_or_else(|| {
                self.active.push(Active {
                    index,
                    remaining: 1.0,
                    infinite: true,
                    debuff: false,
                    icon: Some(String::new()),
                    since_cycle: 0.0,
                    cycled: false,
                    invulnerable: e.invulnerable,
                });
                self.active.len() - 1
            });
            self.active[i].infinite = true;
            let u = self.active[i].update(true);
            self.changes.push(u);
            return new;
        }
        let duration = duration.unwrap_or(e.duration);
        match at {
            Some(i) => {
                let a = &mut self.active[i];
                if a.infinite || e.overlap == Overlap::IGNORE {
                    return false;
                }
                a.remaining = if e.overlap == Overlap::EXTEND {
                    a.remaining + duration
                } else {
                    duration
                };
                let u = a.update(false);
                self.changes.push(u);
                false
            }
            None => {
                let a = Active {
                    index,
                    remaining: duration,
                    infinite: false,
                    debuff: e.debuff,
                    icon: e.icon.clone(),
                    since_cycle: 0.0,
                    cycled: false,
                    invulnerable: e.invulnerable,
                };
                self.changes.push(a.update(false));
                self.active.push(a);
                true
            }
        }
    }

    /// Removes an effect the way `how` says. True when it left.
    pub fn remove(&mut self, index: i32, how: Removal) -> bool {
        let Some(i) = self.active.iter().position(|a| a.index == index) else {
            return false;
        };
        match how {
            Removal::Complete => {
                self.active.remove(i);
                self.changes.push(p::EntityEffectUpdate {
                    r#type: p::EffectOp::REMOVE,
                    id: index,
                    remaining_time: 0.0,
                    infinite: false,
                    debuff: false,
                    status_effect_icon: Some(String::new()),
                });
                return true;
            }
            Removal::Infinite => self.active[i].infinite = false,
            Removal::Duration => self.active[i].remaining = 0.0,
        }
        let mut u = self.active[i].update(self.active[i].infinite);
        u.r#type = p::EffectOp::REMOVE;
        self.changes.push(u);
        false
    }

    /// One tick: every effect whose conditions still hold (`ok`) runs its
    /// cycle — `change(stats, percent)` applies a cycle's stat changes —
    /// and loses time; the ones that ran out or no longer apply leave.
    /// True when an effect left.
    pub fn tick(
        &mut self,
        book: &Book,
        dt: f32,
        mut ok: impl FnMut(&Effect) -> bool,
        mut change: impl FnMut(&[(i32, f32)], bool),
    ) -> bool {
        if self.active.is_empty() {
            return false;
        }
        let mut gone = Vec::new();
        let mut invulnerable = false;
        for index in self.order() {
            let Some(a) = self.active.iter_mut().find(|a| a.index == index) else {
                continue;
            };
            let Some(e) = book.get(index).filter(|e| ok(e)) else {
                gone.push(index);
                continue;
            };
            let dt = a.remaining.min(dt);
            let cycles = if e.cooldown > 0.0 {
                a.since_cycle += dt;
                let n = (a.since_cycle / e.cooldown).floor() as i32;
                a.since_cycle %= e.cooldown;
                n
            } else if !a.cycled {
                a.cycled = true;
                1
            } else {
                0
            };
            if cycles > 0 && !e.stats.is_empty() {
                change(&e.stats, e.percent);
            }
            if !a.infinite {
                a.remaining -= dt;
                if a.remaining <= 0.0 {
                    gone.push(index);
                }
            }
            invulnerable |= a.invulnerable;
        }
        self.invulnerable = invulnerable;
        for i in &gone {
            self.remove(*i, Removal::Complete);
        }
        !gone.is_empty()
    }

    /// The modifiers active effects put on stat maximums: (stat,
    /// multiplicative, amount).
    pub fn modifiers(&self, book: &Book) -> Vec<(i32, bool, f32)> {
        self.active
            .iter()
            .filter(|a| a.infinite || a.remaining > 0.0)
            .filter_map(|a| book.get(a.index))
            .flat_map(|e| e.modifiers.iter().copied())
            .collect()
    }

    /// The changes since the last call, for the entity's viewers.
    pub fn take_changes(&mut self) -> Option<p::EntityEffectsUpdate> {
        if self.changes.is_empty() {
            return None;
        }
        Some(p::EntityEffectsUpdate {
            entity_effect_updates: std::mem::take(&mut self.changes),
        })
    }

    /// Everything active, for a viewer that starts seeing the entity.
    #[allow(dead_code)]
    pub fn init(&self) -> p::EntityEffectsUpdate {
        p::EntityEffectsUpdate {
            entity_effect_updates: self
                .order()
                .into_iter()
                .filter_map(|i| self.active.iter().find(|a| a.index == i))
                .map(|a| a.update(a.infinite))
                .collect(),
        }
    }
}
