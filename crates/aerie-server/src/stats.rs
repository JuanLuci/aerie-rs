//! Entity stats that change (spec 17): health, stamina, mana and the
//! rest. Each entity has a value per stat type, kept within the type's
//! minimum and maximum (moved by modifiers); changes are recorded as the
//! updates clients get, merged as the official server merges them, and
//! stats regenerate by the rules in their assets.

use std::collections::HashMap;

use aerie_protocol::proto;
use serde_json::Value;

pub const TABLE: &str = "stats";

/// One condition of a regeneration rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub inverse: bool,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Alive,
    IsPlayer,
    /// No damage taken for this many seconds.
    NoDamageTaken(f32),
    RegenHealth,
    /// The player's game mode is creative (`true`) or adventure.
    GameMode(bool),
    Stat {
        stat: String,
        amount: f32,
        cmp: Cmp,
    },
    Wielding {
        use_ignore_flag: bool,
    },
    Sprinting,
    Gliding,
    Suffocating,
    /// A condition Aerie does not know: never met.
    Unknown(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmp {
    Gte,
    Gt,
    Lte,
    Lt,
    Equal,
}

impl Cmp {
    fn test(self, a: f32, b: f32) -> bool {
        match self {
            Cmp::Gte => a >= b,
            Cmp::Gt => a > b,
            Cmp::Lte => a <= b,
            Cmp::Lt => a < b,
            Cmp::Equal => a == b,
        }
    }
}

/// What conditions ask about an entity.
pub trait Who {
    fn alive(&self) -> bool {
        true
    }
    fn is_player(&self) -> bool {
        true
    }
    /// Seconds since it last took damage (`None`: never).
    fn since_damage(&self) -> Option<f32> {
        None
    }
    fn regen_health(&self) -> bool {
        true
    }
    fn creative(&self) -> bool;
    /// Blocking with a weapon or shield; `ignored`: the wielding
    /// interaction is flagged to be ignored by conditions.
    fn wielding(&self) -> Option<bool> {
        None
    }
    fn sprinting(&self) -> bool;
    fn gliding(&self) -> bool;
    fn suffocating(&self) -> bool {
        false
    }
}

/// One `Regenerating` entry of a stat type.
#[derive(Clone, Debug, PartialEq)]
pub struct Regen {
    pub interval: f32,
    pub amount: f32,
    pub percentage: bool,
    pub clamp_at_zero: bool,
    pub conditions: Vec<Cond>,
    /// Multipliers applied while their conditions are met.
    pub modifiers: Vec<(Vec<Cond>, f32)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Type {
    pub key: String,
    pub initial: f32,
    pub min: f32,
    pub max: f32,
    /// Other players see it.
    pub shared: bool,
    /// Reset goes to the maximum (else to the initial value).
    pub reset_max: bool,
    pub ignore_invulnerability: bool,
    pub regen: Vec<Regen>,
}

/// The stat types, by index.
#[derive(Clone, Debug, Default)]
pub struct Types {
    pub types: Vec<Type>,
    by_key: HashMap<String, usize>,
}

fn conds(v: Option<&Value>) -> Vec<Cond> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| {
            let s = |k: &str| c.get(k).and_then(Value::as_str);
            let f = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let kind = match s("Id").unwrap_or("") {
                "Alive" => Kind::Alive,
                "IsPlayer" => Kind::IsPlayer,
                "NoDamageTaken" => Kind::NoDamageTaken(f("Delay")),
                "RegenHealth" => Kind::RegenHealth,
                "CheckPlayerGameMode" => Kind::GameMode(s("GameMode") == Some("Creative")),
                "Stat" => Kind::Stat {
                    stat: s("Stat").unwrap_or("").to_owned(),
                    amount: f("Amount"),
                    cmp: match s("Comparison")
                        .unwrap_or("Gte")
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "gt" => Cmp::Gt,
                        "lte" => Cmp::Lte,
                        "lt" => Cmp::Lt,
                        "equal" => Cmp::Equal,
                        _ => Cmp::Gte,
                    },
                },
                "Wielding" => Kind::Wielding {
                    use_ignore_flag: c.get("UseIgnoreFlag").and_then(Value::as_bool) == Some(true),
                },
                "Sprinting" => Kind::Sprinting,
                "Gliding" => Kind::Gliding,
                "Suffocating" => Kind::Suffocating,
                other => Kind::Unknown(other.to_owned()),
            };
            Cond {
                inverse: c.get("Inverse").and_then(Value::as_bool) == Some(true),
                kind,
            }
        })
        .collect()
}

impl Types {
    /// Parses the stats table (aerie-assets).
    pub fn parse(text: &str) -> Self {
        let mut t = Types::default();
        for l in text.lines() {
            let mut w = l.splitn(7, ' ');
            match w.next() {
                Some("S") => {
                    let v: Vec<&str> = l.split_whitespace().collect();
                    if let [_, i, key, initial, min, max, shared, reset, ignore] = v[..] {
                        let i: usize = i.parse().unwrap_or(0);
                        if t.types.len() <= i {
                            t.types.resize(i + 1, Type::default());
                        }
                        t.types[i] = Type {
                            key: key.to_owned(),
                            initial: initial.parse().unwrap_or(0.0),
                            min: min.parse().unwrap_or(0.0),
                            max: max.parse().unwrap_or(0.0),
                            shared: shared == "1",
                            reset_max: reset == "1",
                            ignore_invulnerability: ignore == "1",
                            regen: Vec::new(),
                        };
                        t.by_key.insert(key.to_owned(), i);
                    }
                }
                Some("R") => {
                    let (Some(i), Some(interval), Some(amount), Some(pct), Some(clamp), Some(json)) =
                        (w.next(), w.next(), w.next(), w.next(), w.next(), w.next())
                    else {
                        continue;
                    };
                    let extra: Value = serde_json::from_str(json).unwrap_or(Value::Null);
                    let Some(ty) = i.parse::<usize>().ok().and_then(|i| t.types.get_mut(i)) else {
                        continue;
                    };
                    ty.regen.push(Regen {
                        interval: interval.parse().unwrap_or(0.0),
                        amount: amount.parse().unwrap_or(0.0),
                        percentage: pct == "1",
                        clamp_at_zero: clamp == "1",
                        conditions: conds(extra.get("Conditions")),
                        modifiers: extra
                            .get("Modifiers")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(|m| {
                                (
                                    conds(m.get("Conditions")),
                                    m.get("Amount").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                                )
                            })
                            .collect(),
                    });
                }
                _ => {}
            }
        }
        t
    }

    pub fn index(&self, key: &str) -> Option<usize> {
        self.by_key.get(key).copied()
    }
}

/// A modifier of a stat's minimum or maximum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Modifier {
    pub max: bool,
    pub multiplicative: bool,
    pub amount: f32,
}

impl Modifier {
    fn packet(&self) -> proto::Modifier {
        proto::Modifier {
            target: if self.max {
                proto::ModifierTarget::MAX
            } else {
                proto::ModifierTarget::MIN
            },
            calculation_type: if self.multiplicative {
                proto::CalculationType::MULTIPLICATIVE
            } else {
                proto::CalculationType::ADDITIVE
            },
            amount: self.amount,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct StatValue {
    value: f32,
    min: f32,
    max: f32,
    /// Seconds until each regeneration rule next applies.
    timers: Vec<f32>,
    modifiers: Vec<(String, Modifier)>,
}

impl StatValue {
    fn set(&mut self, v: f32) -> f32 {
        self.value = v.clamp(self.min, self.max.max(self.min));
        self.value
    }

    /// Minimum and maximum from the type and the modifiers (per target:
    /// the additive amounts summed and added, then the multiplicative
    /// amounts summed and multiplied), the value kept inside.
    fn compute(&mut self, ty: &Type) {
        self.min = ty.min;
        self.max = ty.max;
        for max in [false, true] {
            let of = |mult: bool| -> Option<f32> {
                let v: Vec<f32> = self
                    .modifiers
                    .iter()
                    .filter(|(_, m)| m.max == max && m.multiplicative == mult)
                    .map(|(_, m)| m.amount)
                    .collect();
                (!v.is_empty()).then(|| v.iter().sum())
            };
            let (add, mul) = (of(false), of(true));
            let t = if max { &mut self.max } else { &mut self.min };
            if let Some(a) = add {
                *t += a;
            }
            if let Some(m) = mul {
                *t *= m;
            }
        }
        self.value = self.value.clamp(self.min, self.max.max(self.min));
    }
}

/// How far a change is predicted by clients.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Predictable {
    None,
    /// By the entity's own client.
    Own,
    All,
}

type Update = proto::EntityStatUpdate;

fn update(op: proto::EntityStatOp, predictable: bool, value: f32) -> Update {
    Update {
        op,
        predictable,
        value,
        modifiers: None,
        modifier_key: None,
        modifier: None,
    }
}

/// The official merge of a new change into the pending ones: `true` when
/// it went into the last one.
fn merge(list: &mut Vec<Update>, op: proto::EntityStatOp, value: f32, predictable: bool) -> bool {
    use proto::EntityStatOp as Op;
    let last = list.last_mut();
    match op {
        Op::ADD => match last {
            Some(l)
                if !predictable
                    && !l.predictable
                    && matches!(l.op, Op::INIT | Op::ADD | Op::SET) =>
            {
                l.value += value;
                true
            }
            _ => {
                list.push(update(op, predictable, value));
                false
            }
        },
        Op::SET | Op::MINIMIZE | Op::MAXIMIZE | Op::RESET => match last {
            Some(l) if !predictable && !l.predictable && l.op != Op::REMOVE => {
                if l.op != Op::INIT {
                    l.op = op;
                }
                l.value = value;
                true
            }
            _ => {
                list.push(update(op, predictable, value));
                false
            }
        },
        _ => {
            list.push(update(op, predictable, value));
            false
        }
    }
}

/// An entity's stats.
#[derive(Clone, Debug, Default)]
pub struct StatMap {
    values: Vec<StatValue>,
    /// Pending updates per stat: for the entity's own client, and for
    /// the others (shared stats only).
    own: Vec<Vec<Update>>,
    others: Vec<Vec<Update>>,
}

impl StatMap {
    pub fn new(types: &Types) -> Self {
        let values: Vec<StatValue> = types
            .types
            .iter()
            .map(|t| StatValue {
                value: t.initial.clamp(t.min, t.max.max(t.min)),
                min: t.min,
                max: t.max,
                timers: vec![0.0; t.regen.len()],
                modifiers: Vec::new(),
            })
            .collect();
        let n = values.len();
        StatMap {
            values,
            own: vec![Vec::new(); n],
            others: vec![Vec::new(); n],
        }
    }

    pub fn get(&self, i: usize) -> Option<f32> {
        self.values.get(i).map(|v| v.value)
    }

    pub fn max(&self, i: usize) -> Option<f32> {
        self.values.get(i).map(|v| v.max)
    }

    fn change(
        &mut self,
        types: &Types,
        i: usize,
        op: proto::EntityStatOp,
        value: f32,
        p: Predictable,
    ) {
        if types.types[i].shared {
            merge(&mut self.others[i], op, value, p == Predictable::All);
        }
        merge(&mut self.own[i], op, value, p != Predictable::None);
    }

    /// Adds to a stat (kept within its bounds); the new value.
    pub fn add(&mut self, types: &Types, i: usize, amount: f32, p: Predictable) -> f32 {
        let Some(v) = self.values.get_mut(i) else {
            return 0.0;
        };
        let before = v.value;
        let after = v.set(before + amount);
        if p != Predictable::None || after != before {
            self.change(types, i, proto::EntityStatOp::ADD, amount, p);
        }
        after
    }

    pub fn set(&mut self, types: &Types, i: usize, value: f32, p: Predictable) -> f32 {
        let Some(v) = self.values.get_mut(i) else {
            return 0.0;
        };
        let before = v.value;
        let after = v.set(value);
        if p != Predictable::None || value != before {
            self.change(types, i, proto::EntityStatOp::SET, value, p);
        }
        after
    }

    /// To the type's reset value (its maximum or its initial value).
    pub fn reset(&mut self, types: &Types, i: usize) -> f32 {
        let Some(v) = self.values.get_mut(i) else {
            return 0.0;
        };
        let before = v.value;
        let to = if types.types[i].reset_max {
            v.max
        } else {
            types.types[i].initial
        };
        let after = v.set(to);
        if after != before {
            self.change(types, i, proto::EntityStatOp::RESET, 0.0, Predictable::None);
        }
        after
    }

    /// To the stat's minimum.
    pub fn minimize(&mut self, types: &Types, i: usize, p: Predictable) -> f32 {
        let Some(v) = self.values.get_mut(i) else {
            return 0.0;
        };
        let before = v.value;
        let after = v.set(v.min);
        if p != Predictable::None || after != before {
            self.change(types, i, proto::EntityStatOp::MINIMIZE, 0.0, p);
        }
        after
    }

    pub fn modifier(&self, i: usize, key: &str) -> Option<Modifier> {
        self.values
            .get(i)?
            .modifiers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, m)| *m)
    }

    /// Puts a modifier under `key` (replacing one there).
    pub fn put_modifier(
        &mut self,
        types: &Types,
        i: usize,
        key: &str,
        m: Modifier,
        p: Predictable,
    ) {
        let Some(v) = self.values.get_mut(i) else {
            return;
        };
        match v.modifiers.iter_mut().find(|(k, _)| k == key) {
            Some(x) => x.1 = m,
            None => v.modifiers.push((key.to_owned(), m)),
        }
        v.compute(&types.types[i]);
        let u = Update {
            modifier_key: Some(key.to_owned()),
            modifier: Some(m.packet()),
            ..update(
                proto::EntityStatOp::PUT_MODIFIER,
                p != Predictable::None,
                0.0,
            )
        };
        if types.types[i].shared {
            self.others[i].push(Update {
                predictable: p == Predictable::All,
                ..u.clone()
            });
        }
        self.own[i].push(u);
    }

    /// Removes the modifier under `key`; `false` if there was none.
    pub fn remove_modifier(&mut self, types: &Types, i: usize, key: &str, p: Predictable) -> bool {
        let Some(v) = self.values.get_mut(i) else {
            return false;
        };
        let Some(at) = v.modifiers.iter().position(|(k, _)| k == key) else {
            return false;
        };
        v.modifiers.remove(at);
        v.compute(&types.types[i]);
        let u = Update {
            modifier_key: Some(key.to_owned()),
            ..update(
                proto::EntityStatOp::REMOVE_MODIFIER,
                p != Predictable::None,
                0.0,
            )
        };
        if types.types[i].shared {
            self.others[i].push(Update {
                predictable: p == Predictable::All,
                ..u.clone()
            });
        }
        self.own[i].push(u);
        true
    }

    /// The held armor's modifiers (the official recalculation): per stat,
    /// the additive and the multiplicative amounts of every piece summed
    /// into one modifier each on the maximum, under `<source>_ADDITIVE`
    /// and `<source>_MULTIPLICATIVE`; a kind no piece gives is removed.
    /// `sums`: (stat, multiplicative, amount).
    pub fn apply_summed(&mut self, types: &Types, source: &str, sums: &[(i32, bool, f32)]) {
        for i in 0..self.values.len() {
            for (mult, name) in [(false, "ADDITIVE"), (true, "MULTIPLICATIVE")] {
                let key = format!("{source}_{name}");
                let total: Option<f32> = {
                    let v: Vec<f32> = sums
                        .iter()
                        .filter(|(s, m, _)| *s == i as i32 && *m == mult)
                        .map(|(_, _, a)| *a)
                        .collect();
                    (!v.is_empty()).then(|| v.iter().sum())
                };
                match total {
                    None => {
                        self.remove_modifier(types, i, &key, Predictable::None);
                    }
                    Some(amount) => self.put_modifier(
                        types,
                        i,
                        &key,
                        Modifier {
                            max: true,
                            multiplicative: mult,
                            amount,
                        },
                        Predictable::None,
                    ),
                }
            }
        }
    }

    /// A held item's own modifiers (weapon, utility), under
    /// `<prefix><n>` per stat: put when different from what is there, the
    /// keys past the item's count and the stats it does not touch
    /// cleared; all predicted by the holder's client.
    pub fn apply_item(
        &mut self,
        types: &Types,
        prefix: &str,
        mods: Option<&[(i32, Vec<Modifier>)]>,
    ) {
        let clear_from = |m: &mut StatMap, i: usize, mut n: usize| {
            while m.remove_modifier(types, i, &format!("{prefix}{n}"), Predictable::Own) {
                n += 1;
            }
        };
        let touched: Vec<i32> = mods.into_iter().flatten().map(|(i, _)| *i).collect();
        for (i, ms) in mods.into_iter().flatten() {
            let i = *i as usize;
            for (n, m) in ms.iter().enumerate() {
                let key = format!("{prefix}{n}");
                if self.modifier(i, &key) != Some(*m) {
                    self.put_modifier(types, i, &key, *m, Predictable::Own);
                }
            }
            clear_from(self, i, ms.len());
        }
        for i in 0..self.values.len() {
            if !touched.contains(&(i as i32)) {
                clear_from(self, i, 0);
            }
        }
    }

    fn met(&self, types: &Types, who: &impl Who, conds: &[Cond]) -> bool {
        conds.iter().all(|c| {
            let v = match &c.kind {
                Kind::Alive => who.alive(),
                Kind::IsPlayer => who.is_player(),
                Kind::NoDamageTaken(delay) => who.since_damage().is_none_or(|s| s >= *delay),
                Kind::RegenHealth => who.regen_health(),
                Kind::GameMode(creative) => who.is_player() && who.creative() == *creative,
                Kind::Stat { stat, amount, cmp } => types
                    .index(stat)
                    .and_then(|i| self.get(i))
                    .is_some_and(|v| cmp.test(v, *amount)),
                Kind::Wielding { use_ignore_flag } => who
                    .wielding()
                    .is_some_and(|ignored| !*use_ignore_flag || !ignored),
                Kind::Sprinting => who.sprinting(),
                Kind::Gliding => who.gliding(),
                Kind::Suffocating => who.suffocating(),
                Kind::Unknown(_) => false,
            };
            v != c.inverse
        })
    }

    /// One tick of regeneration (the official order: every stat but index
    /// 0, each rule in order while the stat is not already at the end the
    /// rule moves it to; a rule's timer runs down by `dt` and, below zero,
    /// goes up by its interval and applies if its conditions are met).
    pub fn regenerate(&mut self, types: &Types, who: &impl Who, dt: f32, invulnerable: bool) {
        let n = self.values.len();
        let mut sums = vec![0.0f32; n];
        for i in 1..n {
            for (k, r) in types.types[i].regen.iter().enumerate() {
                let v = &self.values[i];
                let room = if r.amount > 0.0 {
                    !(v.value >= v.max)
                } else {
                    !(v.value <= v.min)
                };
                if !room {
                    continue;
                }
                let t = &mut self.values[i].timers[k];
                *t -= dt;
                if *t >= 0.0 {
                    continue;
                }
                *t += r.interval;
                if !self.met(types, who, &r.conditions) {
                    continue;
                }
                let v = &self.values[i];
                let mut add = if r.percentage {
                    r.amount * (v.max - v.min)
                } else {
                    r.amount
                };
                for (c, m) in &r.modifiers {
                    if self.met(types, who, c) {
                        add *= m;
                    }
                }
                if r.clamp_at_zero && !(add > 0.0) && v.min < 0.0 {
                    let result = v.value + sums[i] + add;
                    if result < 0.0 {
                        add = (add - result).min(0.0);
                    }
                }
                sums[i] += add;
            }
        }
        for i in 1..n {
            let a = sums[i];
            if a != 0.0 && (!(a < 0.0) || types.types[i].ignore_invulnerability || !invulnerable) {
                self.add(types, i, a, Predictable::None);
            }
        }
    }

    /// Stat indexes in the order the official server lists them in an
    /// update: its pending-update table (made for 16, filled 0, 1, 2… when
    /// the entity was made) read into a table made for that many entries.
    fn order(&self, pending: &[Vec<Update>]) -> Vec<i32> {
        let n = self.values.len();
        let first = crate::changes::map_order(0..n as i32);
        crate::changes::order_in(
            n,
            first
                .into_iter()
                .filter(|&i| !pending[i as usize].is_empty()),
        )
    }

    fn take(&mut self, own: bool) -> Option<proto::EntityStatsUpdate> {
        let order = self.order(if own { &self.own } else { &self.others });
        if order.is_empty() {
            return None;
        }
        let lists = if own { &mut self.own } else { &mut self.others };
        Some(proto::EntityStatsUpdate {
            entity_stat_updates: order
                .into_iter()
                .map(|i| (i, std::mem::take(&mut lists[i as usize])))
                .collect(),
        })
    }

    /// The pending updates for the entity's own client, and for the
    /// others; both lists are emptied.
    pub fn take_updates(
        &mut self,
    ) -> (
        Option<proto::EntityStatsUpdate>,
        Option<proto::EntityStatsUpdate>,
    ) {
        (self.take(true), self.take(false))
    }

    /// Every stat (`all`) or the shared ones, as a newly seen entity's
    /// first update.
    pub fn init(&self, types: &Types, all: bool) -> proto::EntityStatsUpdate {
        let n = self.values.len();
        let keys = (0..n as i32).filter(|&i| all || types.types[i as usize].shared);
        proto::EntityStatsUpdate {
            entity_stat_updates: crate::changes::order_in(n, keys)
                .into_iter()
                .map(|i| {
                    let v = &self.values[i as usize];
                    let mods: Vec<(String, proto::Modifier)> = v
                        .modifiers
                        .iter()
                        .map(|(k, m)| (k.clone(), m.packet()))
                        .collect();
                    (
                        i,
                        vec![Update {
                            modifiers: (!mods.is_empty()).then_some(mods),
                            ..update(proto::EntityStatOp::INIT, false, v.value)
                        }],
                    )
                })
                .collect(),
        }
    }

    /// Text form for saving: `key value` per line.
    pub fn to_text(&self, types: &Types) -> String {
        self.values
            .iter()
            .zip(&types.types)
            .map(|(v, t)| format!("{} {}\n", t.key, v.value))
            .collect()
    }

    pub fn load(&mut self, types: &Types, text: &str) {
        for l in text.lines() {
            if let Some((k, v)) = l.split_once(' ')
                && let (Some(i), Ok(v)) = (types.index(k), v.parse::<f32>())
            {
                self.values[i].set(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "S 0 Unknown 0 0 0 0 0 0\n\
        S 1 Health 100 0 100 1 1 0\n\
        S 2 Stamina 10 -4 10 0 0 0\n\
        R 2 0.1 0.3 0 0 {\"Conditions\":[{\"Id\":\"Stat\",\"Stat\":\"Delay\",\"Amount\":0},{\"Id\":\"Sprinting\",\"Inverse\":true}],\"Modifiers\":null}\n\
        S 3 Delay 0 -60 0 0 0 0\n\
        R 3 0.1 0.1 0 0 {\"Conditions\":null,\"Modifiers\":null}\n";

    struct P(bool);
    impl Who for P {
        fn creative(&self) -> bool {
            false
        }
        fn sprinting(&self) -> bool {
            self.0
        }
        fn gliding(&self) -> bool {
            false
        }
    }

    #[test]
    fn changes_merge_and_regenerate() {
        let t = Types::parse(T);
        let mut m = StatMap::new(&t);
        // A command takes 5 stamina; the tick's regeneration merges in.
        assert_eq!(m.add(&t, 2, -5.0, Predictable::None), 5.0);
        m.regenerate(&t, &P(false), 1.0 / 30.0, false);
        let (own, others) = m.take_updates();
        let own = own.unwrap().entity_stat_updates;
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].0, 2);
        assert_eq!(own[0].1[0].op, proto::EntityStatOp::ADD);
        assert!((own[0].1[0].value + 4.7).abs() < 1e-5);
        // Stamina is not shared.
        assert!(others.is_none());
        // Sprinting: no regeneration.
        m.regenerate(&t, &P(true), 0.2, false);
        assert!(m.take_updates().0.is_none());
        // Health is shared; a set after an add becomes one set.
        m.add(&t, 1, -30.0, Predictable::None);
        m.set(&t, 1, 40.0, Predictable::None);
        let (own, others) = m.take_updates();
        let h = &own.unwrap().entity_stat_updates[0].1;
        assert_eq!(
            (h.len(), h[0].op, h[0].value),
            (1, proto::EntityStatOp::SET, 40.0)
        );
        assert!(others.is_some());
    }

    #[test]
    fn modifiers_move_the_bounds() {
        let t = Types::parse(T);
        let mut m = StatMap::new(&t);
        let add = Modifier {
            max: true,
            multiplicative: false,
            amount: 20.0,
        };
        m.put_modifier(&t, 1, "Armor", add, Predictable::None);
        assert_eq!(m.max(1), Some(120.0));
        assert!(m.remove_modifier(&t, 1, "Armor", Predictable::None));
        // Two armor pieces summed into one modifier; then none.
        m.apply_summed(&t, "Armor", &[(1, false, 5.0), (1, false, 7.0)]);
        assert_eq!(m.max(1), Some(112.0));
        m.apply_summed(&t, "Armor", &[]);
        assert_eq!(m.max(1), Some(100.0));
        // A weapon's own modifiers, then swapped away.
        m.apply_item(&t, "*Weapon_", Some(&[(2, vec![add])]));
        assert_eq!(m.max(2), Some(30.0));
        m.apply_item(&t, "*Weapon_", None);
        assert_eq!(m.max(2), Some(10.0));
        m.take_updates();
        assert_eq!((m.max(1), m.get(1)), (Some(100.0), Some(100.0)));
    }

    #[test]
    fn init_lists_stats_in_the_official_order() {
        let mut text = String::new();
        for i in 0..13 {
            text += &format!("S {i} S{i} 0 0 0 1 0 0\n");
        }
        let t = Types::parse(&text);
        let m = StatMap::new(&t);
        let order: Vec<i32> = m
            .init(&t, true)
            .entity_stat_updates
            .iter()
            .map(|e| e.0)
            .collect();
        // As captured from the official server at join.
        assert_eq!(order, [0, 2, 6, 4, 12, 8, 9, 11, 10, 1, 3, 7, 5]);
    }
}
