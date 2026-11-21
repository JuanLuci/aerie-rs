//! Weapon damage breakdowns (`ItemWeapon.basic_damage_breakdown` for the
//! `Primary` chain, `ultimate_damage_breakdown` for `Ability1`), computed by
//! walking the item's interaction chain the way the official server's
//! damage collector does (spec 07, "Items").
//!
//! The walk starts at the root's interactions and follows each type's links
//! in order (`Next`/`Failed` for most; `Chaining` and `Serial` lists,
//! `Charging` by time then `Failed`, `Parallel` and `Selector` roots, a
//! `Replace` through the item's `InteractionVars`, `DamageEntity` through
//! `Next`, `Failed`, `Blocked`, then each angled and targeted entry). Every
//! step carries the damage summed along its path and a label: `charged`
//! after a charge longer than 0 s, `backstab` after an angled entry,
//! `targeted.<key>` after a targeted one, none after `Failed`/`Blocked` or a
//! projectile. A `DamageEntity` adds its calculator's range (sum of
//! `BaseDamage`, times `RunTime` for `DPS`, ± `RandomPercentageModifier`);
//! leaving it by `Failed`/`Blocked` takes it back off, and by an angled or
//! targeted entry swaps in that entry's calculator. A `Projectile` would add
//! the damage range of its config's `ProjectileHit` chain, but the official
//! server computes this before projectile configs load, so it adds nothing. An interaction seen
//! twice on one path ends that path. Each path end with damage is a leaf;
//! leaves are grouped by label (min of mins, max of maxes), unlabelled
//! first (or `charged` when there is no unlabelled one).

use std::collections::HashMap;

use aerie_protocol::proto as p;
use serde_json::Value;

use super::music::{owner_of, reference};
use super::root_interactions::{self, INTERACTIONS};
use crate::Result;
use crate::ctx::Ctx;
use crate::store::Store;
use crate::values::Get;

pub const PROJECTILES: &str = "ProjectileConfigs";

/// Deep enough for every real chain; deeper means a cycle, which the
/// official server ends with a stack overflow (and no data).
const MAX_DEPTH: usize = 400;

/// The official server computes these while items load, before projectile
/// configs are loaded, so a projectile's hit adds nothing.
const PROJECTILE_HITS: bool = false;

#[derive(Clone, Copy, PartialEq)]
enum Tag<'a> {
    None,
    Next,
    Failed,
    Blocked,
    Charging(f64),
    Angled(usize),
    Targeted(&'a str),
}

#[derive(Clone, Default)]
struct Frame {
    min: f32,
    max: f32,
    label: Option<String>,
    pending: Option<String>,
    visited: Vec<String>,
    descended: bool,
    aborted: bool,
}

struct Walk<'a> {
    ctx: &'a Ctx,
    inter: &'a Store,
    roots: &'a Store,
    vars: &'a HashMap<String, String>,
    stack: Vec<Frame>,
    leaves: Vec<(Option<String>, f32, f32)>,
    /// Hit collector mode (projectile hits): no labels, one range.
    hit: bool,
    overflow: bool,
}

fn calc_range(c: Option<&Value>, run_time: f64) -> (f32, f32) {
    let Some(c) = c else { return (0.0, 0.0) };
    let Some(base) = c
        .get("BaseDamage")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
    else {
        return (0.0, 0.0);
    };
    let dps = c
        .str_at("Type")
        .is_some_and(|t| t.eq_ignore_ascii_case("DPS"));
    let mut scaled = 0.0f32;
    for v in base.values() {
        let d = v.as_f64().unwrap_or(0.0) as f32;
        scaled += if dps { run_time as f32 * d } else { d };
    }
    let r = c.f32_or("RandomPercentageModifier", 0.0);
    (scaled * (1.0 - r), scaled * (1.0 + r))
}

impl<'a> Walk<'a> {
    fn resolved(&self, k: &str) -> Value {
        if self.inter.own.contains_key(k) {
            self.inter.resolved(k, "Interaction")
        } else {
            Value::Null
        }
    }

    fn type_of(&self, k: &str) -> String {
        super::interactions::type_of(self.inter, k)
    }

    fn link(&self, k: &str, a: &Value, field: &str, path: &str) -> Option<String> {
        let v = a.get(field)?;
        reference(v, &owner_of(self.inter, k, field), path)
    }

    fn root_ids(&self, root: Option<&str>) -> Vec<String> {
        let Some(r) = root else { return Vec::new() };
        if !self.roots.own.contains_key(r) {
            return Vec::new();
        }
        let a = self.roots.resolved(r, "RootInteraction");
        root_interactions::interaction_keys(self.roots, r, &a)
    }

    fn add(&self, f: &mut Frame, calc: Option<&Value>, run_time: f64, sign: f32) {
        let (lo, hi) = calc_range(calc, run_time);
        f.min += sign * lo;
        f.max += sign * hi;
    }

    fn adjust(&self, parent: &str, tag: Tag, f: &mut Frame) {
        let a = self.resolved(parent);
        let run = a.f64_or("RunTime", 0.0);
        let base = a.get("DamageCalculator");
        match tag {
            Tag::Angled(i) => {
                self.add(f, base, run, -1.0);
                if let Some(e) = a
                    .get("AngledDamage")
                    .and_then(Value::as_array)
                    .and_then(|l| l.get(i))
                {
                    self.add(f, e.get("DamageCalculator").or(base), run, 1.0);
                }
            }
            Tag::Targeted(key) => {
                self.add(f, base, run, -1.0);
                if let Some(e) = a.obj("TargetedDamage").and_then(|m| m.get(key)) {
                    self.add(f, e.get("DamageCalculator").or(base), run, 1.0);
                }
            }
            Tag::Failed | Tag::Blocked => self.add(f, base, run, -1.0),
            _ => {}
        }
    }

    fn enter(&mut self, tag: Tag, k: Option<&str>) {
        let parent = self.stack.last().cloned();
        let mut f = match &parent {
            Some(p) => Frame {
                descended: false,
                ..p.clone()
            },
            None => Frame::default(),
        };
        if !self.hit {
            match tag {
                Tag::Charging(s) => f.label = (s > 0.0).then(|| "charged".into()),
                Tag::Angled(_) => f.label = Some("backstab".into()),
                Tag::Targeted(t) => f.label = Some(format!("targeted.{t}")),
                Tag::Failed | Tag::Blocked => f.label = None,
                _ => {}
            }
        }
        if let Some(k) = k {
            if f.visited.iter().any(|v| v == k) {
                f.aborted = true;
            } else {
                f.visited.push(k.to_owned());
                if let Some(pd) = parent.as_ref().and_then(|p| p.pending.clone()) {
                    self.adjust(&pd, tag, &mut f);
                }
            }
            let ty = self.type_of(k);
            if ty == "DamageEntity" {
                let a = self.resolved(k);
                self.add(
                    &mut f,
                    a.get("DamageCalculator"),
                    a.f64_or("RunTime", 0.0),
                    1.0,
                );
                f.pending = Some(k.to_owned());
            } else {
                f.pending = None;
                if ty == "Projectile" && !self.hit {
                    f.label = None;
                    if PROJECTILE_HITS {
                        let (lo, hi) = self.projectile_hit(k);
                        f.min += lo;
                        f.max += hi;
                    }
                }
            }
        }
        if let Some(p) = self.stack.last_mut() {
            p.descended = true;
        }
        self.stack.push(f);
    }

    fn outof(&mut self) {
        let f = self.stack.pop().unwrap_or_default();
        if !f.aborted && !f.descended && f.max > 0.0 {
            self.leaves.push((f.label, f.min, f.max));
        }
    }

    fn projectile_hit(&mut self, k: &str) -> (f32, f32) {
        let Some(cfg) = self.resolved(k).str_at("Config") else {
            return (0.0, 0.0);
        };
        let Ok(store) = self.ctx.store(PROJECTILES) else {
            return (0.0, 0.0);
        };
        if !store.own.contains_key(&cfg) {
            return (0.0, 0.0);
        }
        let c = store.resolved(&cfg, "ProjectileConfig");
        let Some(v) = c.obj("Interactions").and_then(|m| m.get("ProjectileHit")) else {
            return (0.0, 0.0);
        };
        let root = reference(
            v,
            &owner_of(&store, &cfg, "Interactions"),
            "Interactions_ProjectileHit",
        );
        let ids = self.root_ids(root.as_deref());
        let mut sub = Walk {
            ctx: self.ctx,
            inter: self.inter,
            roots: self.roots,
            vars: self.vars,
            stack: Vec::new(),
            leaves: Vec::new(),
            hit: true,
            overflow: false,
        };
        sub.enter(Tag::None, None);
        sub.walk_list(&ids, Tag::None);
        sub.outof();
        self.overflow |= sub.overflow;
        if sub.leaves.is_empty() {
            return (0.0, 0.0);
        }
        let lo = sub.leaves.iter().map(|l| l.1).fold(f32::INFINITY, f32::min);
        let hi = sub
            .leaves
            .iter()
            .map(|l| l.2)
            .fold(f32::NEG_INFINITY, f32::max);
        (lo, hi)
    }

    fn walk_list(&mut self, ids: &[String], tag: Tag) {
        for id in ids {
            self.walk_one(Some(id), tag);
        }
    }

    fn walk_one(&mut self, id: Option<&str>, tag: Tag) {
        let Some(id) = id else { return };
        if self.stack.len() > MAX_DEPTH {
            self.overflow = true;
            return;
        }
        self.enter(tag, Some(id));
        self.walk_children(id);
        self.outof();
    }

    fn simple(&mut self, k: &str, a: &Value) {
        let next = self.link(k, a, "Next", "Next");
        let failed = self.link(k, a, "Failed", "Failed");
        self.walk_one(next.as_deref(), Tag::Next);
        self.walk_one(failed.as_deref(), Tag::Failed);
    }

    fn walk_children(&mut self, k: &str) {
        if self.overflow {
            return;
        }
        let a = self.resolved(k);
        let owner = |f: &str| owner_of(self.inter, k, f);
        match self.type_of(k).as_str() {
            "ChangeActiveSlot" => {}
            "Chaining" => {
                let o = owner("Next");
                let ids: Vec<Option<String>> = a
                    .get("Next")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(i, x)| reference(x, &o, &format!("Next_{i}")))
                    .collect();
                for id in ids {
                    self.walk_one(id.as_deref(), Tag::None);
                }
            }
            "Serial" => {
                let o = owner("Interactions");
                let ids: Vec<Option<String>> = a
                    .get("Interactions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(i, x)| reference(x, &o, &format!("Interactions_{i}")))
                    .collect();
                for id in ids {
                    self.walk_one(id.as_deref(), Tag::None);
                }
            }
            "Parallel" => {
                let o = owner("Interactions");
                let roots: Vec<Option<String>> = a
                    .get("Interactions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(i, x)| reference(x, &o, &format!("Interactions_{i}")))
                    .collect();
                for r in roots {
                    let ids = self.root_ids(r.as_deref());
                    self.walk_list(&ids, Tag::None);
                }
            }
            "Charging" => {
                let o = owner("Next");
                let entries: Vec<(f64, Option<String>)> = a
                    .obj("Next")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .map(|(s, x)| {
                        (
                            s.parse().unwrap_or(0.0),
                            reference(x, &o, &format!("Next_{s}")),
                        )
                    })
                    .collect();
                for (s, id) in entries {
                    self.walk_one(id.as_deref(), Tag::Charging(s));
                }
                let failed = self.link(k, &a, "Failed", "Failed");
                self.walk_one(failed.as_deref(), Tag::Failed);
            }
            "FirstClick" => {
                let click = self.link(k, &a, "Click", "Click");
                let held = self.link(k, &a, "Held", "Held");
                self.walk_one(click.as_deref(), Tag::None);
                self.walk_one(held.as_deref(), Tag::None);
            }
            "Replace" => {
                let var = a.str_at("Var").and_then(|v| self.vars.get(&v).cloned());
                let root = var.or_else(|| self.link(k, &a, "DefaultValue", "DefaultValue"));
                let ids = self.root_ids(root.as_deref());
                self.walk_list(&ids, Tag::None);
            }
            "Repeat" | "DragPlaceBlock" => {
                let fork = self.link(k, &a, "ForkInteractions", "ForkInteractions");
                let ids = self.root_ids(fork.as_deref());
                self.walk_list(&ids, Tag::None);
                self.simple(k, &a);
            }
            "Selector" => {
                let he = self.link(k, &a, "HitEntity", "HitEntity");
                let ids = self.root_ids(he.as_deref());
                self.walk_list(&ids, Tag::None);
                let o = owner("HitEntityRules");
                let rules: Vec<Option<String>> = a
                    .get("HitEntityRules")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(i, r)| {
                        r.get("Next")
                            .and_then(|n| reference(n, &o, &format!("HitEntityRules_{i}_Next")))
                    })
                    .collect();
                for r in rules {
                    let ids = self.root_ids(r.as_deref());
                    self.walk_list(&ids, Tag::None);
                }
                let hb = self.link(k, &a, "HitBlock", "HitBlock");
                let ids = self.root_ids(hb.as_deref());
                self.walk_list(&ids, Tag::None);
                self.simple(k, &a);
            }
            "DamageEntity" => {
                let next = self.link(k, &a, "Next", "Next");
                let failed = self.link(k, &a, "Failed", "Failed");
                let blocked = self.link(k, &a, "Blocked", "Blocked");
                self.walk_one(next.as_deref(), Tag::Next);
                self.walk_one(failed.as_deref(), Tag::Failed);
                self.walk_one(blocked.as_deref(), Tag::Blocked);
                let o = owner("AngledDamage");
                let angled: Vec<Option<String>> = a
                    .get("AngledDamage")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(i, e)| {
                        e.get("Next")
                            .and_then(|n| reference(n, &o, &format!("AngledDamage_{i}_Next")))
                            .or_else(|| next.clone())
                    })
                    .collect();
                for (i, id) in angled.iter().enumerate() {
                    self.walk_one(id.as_deref(), Tag::Angled(i));
                }
                let o = owner("TargetedDamage");
                let targeted: Vec<(String, Option<String>)> = a
                    .obj("TargetedDamage")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .map(|(t, e)| {
                        (
                            t.clone(),
                            e.get("Next")
                                .and_then(|n| reference(n, &o, &format!("TargetedDamage_{t}_Next")))
                                .or_else(|| next.clone()),
                        )
                    })
                    .collect();
                for (t, id) in &targeted {
                    self.walk_one(id.as_deref(), Tag::Targeted(t));
                }
            }
            _ => self.simple(k, &a),
        }
    }
}

/// The breakdown for the chain an item runs for `ty` (`Primary`,
/// `Ability1`); `root` is that chain's root key, `vars` the item's
/// interaction variables (name → root key).
pub fn breakdown(
    ctx: &Ctx,
    root: Option<&str>,
    vars: &HashMap<String, String>,
) -> Result<p::DamageData> {
    let empty = p::DamageData {
        entries: Some(Vec::new()),
    };
    let inter = ctx.store(INTERACTIONS)?;
    let roots = ctx.store(root_interactions::PATH)?;
    let Some(root) = root.filter(|r| roots.own.contains_key(*r)) else {
        return Ok(empty);
    };
    let mut w = Walk {
        ctx,
        inter: &inter,
        roots: &roots,
        vars,
        stack: Vec::new(),
        leaves: Vec::new(),
        hit: false,
        overflow: false,
    };
    let ids = w.root_ids(Some(root));
    w.enter(Tag::None, None);
    w.walk_list(&ids, Tag::None);
    w.outof();
    if w.overflow || w.leaves.is_empty() {
        return Ok(empty);
    }
    let mut groups: Vec<(String, f32, f32)> = Vec::new();
    for (label, lo, hi) in &w.leaves {
        let key = label.clone().unwrap_or_default();
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => {
                g.1 = g.1.min(*lo);
                g.2 = g.2.max(*hi);
            }
            None => groups.push((key, *lo, *hi)),
        }
    }
    if !groups.iter().any(|g| g.0.is_empty()) {
        if let Some(c) = groups.iter().position(|g| g.0 == "charged") {
            let g = groups.remove(c);
            groups.push((String::new(), g.1, g.2));
        }
    }
    let mut entries = Vec::new();
    if let Some(m) = groups.iter().position(|g| g.0.is_empty()) {
        let g = groups.remove(m);
        entries.push(p::DamageEntry {
            label_key: None,
            min: g.1,
            max: g.2,
        });
    }
    for (k, lo, hi) in groups {
        entries.push(p::DamageEntry {
            label_key: Some(k),
            min: lo,
            max: hi,
        });
    }
    Ok(p::DamageData {
        entries: Some(entries),
    })
}
