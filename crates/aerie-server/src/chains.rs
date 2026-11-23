//! Interaction chains run on the server next to the client (spec 18).
//!
//! [`Book`] is the static side: every interaction and root interaction of
//! the registries, each root compiled to a flat list of operations.
//! [`Manager`] is one entity's running chains: it reads the client's chain
//! packets, starts chains, runs their operations each tick and collects
//! the states to send back. What an operation does to the world goes
//! through [`Host`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use aerie_protocol::proto::{
    self as p, InteractionState as St, InteractionType as IT, WaitForDataFrom as W,
};
use aerie_protocol::wire::Wire;

/// The name of the server types table among the registry extras.
pub const TABLE: &str = "interaction-types";
/// One world tick in nanoseconds, the step of an entity's interaction clock.
pub const TICK_NANOS: i64 = 33_333_333;
const NONE: i32 = i32::MIN;
const TYPES: usize = 28;

pub type Pos = (i32, i32, i32);
/// A step's state, shared with the packets that carry it: a packet shows
/// the state as it is when the tick's packets are written.
pub type Data = Arc<Mutex<p::InteractionSyncData>>;

/// A step state as the official server makes one.
pub fn data(counter: i32, root: i32) -> p::InteractionSyncData {
    p::InteractionSyncData {
        state: St::NOT_FINISHED,
        operation_counter: counter,
        root_interaction: root,
        entered_root_interaction: NONE,
        block_face: p::BlockFace(0),
        placed_block_id: NONE,
        charge_value: -1.0,
        chaining_index: -1,
        flag_index: -1,
        ..Default::default()
    }
}

fn failed(s: St) -> bool {
    matches!(s, St::FAILED | St::SKIP | St::ITEM_CHANGED)
}

/// Interrupt and block rules of a root or an interaction.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    blocked_by: Option<u64>,
    blocking: u64,
    interrupted_by: Option<u64>,
    interrupting: Option<u64>,
    blocked_by_bypass: i32,
    blocking_bypass: i32,
    interrupted_by_bypass: i32,
    interrupting_bypass: i32,
}

fn bits(v: &Option<Vec<IT>>) -> Option<u64> {
    v.as_ref()
        .map(|v| v.iter().fold(0u64, |b, t| b | 1 << (t.0 as u64 & 63)))
}

fn has(set: u64, t: IT) -> bool {
    set & (1 << (t.0 as u64 & 63)) != 0
}

/// What blocks an interaction type when its rules do not say.
fn default_blocked_by(t: IT) -> u64 {
    let standard = bits(&Some(vec![
        IT::PRIMARY,
        IT::SECONDARY,
        IT::ABILITY1,
        IT::ABILITY2,
        IT::ABILITY3,
        IT::ABILITY4,
        IT::USE,
        IT::PICK,
        IT::SWAP_FROM,
        IT::SWAP_TO,
    ]))
    .unwrap_or(0);
    let swap = bits(&Some(vec![IT::SWAP_FROM, IT::SWAP_TO])).unwrap_or(0);
    match t {
        IT::PRIMARY
        | IT::SECONDARY
        | IT::ABILITY1
        | IT::ABILITY2
        | IT::ABILITY3
        | IT::ABILITY4
        | IT::USE
        | IT::PICK => standard,
        IT::SWAP_TO | IT::SWAP_FROM => swap,
        IT::HELD | IT::HELD_OFFHAND | IT::EQUIPPED | IT::DODGE | IT::GAME_MODE_SWAP => {
            1 << t.0 as u64
        }
        _ => 0,
    }
}

impl Rules {
    fn of(r: &p::InteractionRules) -> Rules {
        Rules {
            blocked_by: bits(&r.blocked_by),
            blocking: bits(&r.blocking).unwrap_or(0),
            interrupted_by: bits(&r.interrupted_by),
            interrupting: bits(&r.interrupting),
            blocked_by_bypass: r.blocked_by_bypass_index,
            blocking_bypass: r.blocking_bypass_index,
            interrupted_by_bypass: r.interrupted_by_bypass_index,
            interrupting_bypass: r.interrupting_bypass_index,
        }
    }

    /// Whether a new root with these rules interrupts a running one.
    fn interrupts(
        &self,
        ty: IT,
        tags: &[i32],
        other_ty: IT,
        other_tags: &[i32],
        other: &Rules,
    ) -> bool {
        let by_other = other.interrupted_by.is_some_and(|s| has(s, ty))
            && !(other.interrupted_by_bypass != NONE
                && tags.contains(&other.interrupted_by_bypass));
        by_other
            || (self.interrupting.is_some_and(|s| has(s, other_ty))
                && (self.interrupting_bypass == NONE
                    || !other_tags.contains(&self.interrupting_bypass)))
    }

    /// Whether a running root keeps a new one with these rules from starting.
    fn blocked(
        &self,
        ty: IT,
        tags: &[i32],
        other_ty: IT,
        other_tags: &[i32],
        other: &Rules,
    ) -> bool {
        let set = self.blocked_by.unwrap_or_else(|| default_blocked_by(ty));
        let by_self = has(set, other_ty)
            && !(self.blocked_by_bypass != NONE && other_tags.contains(&self.blocked_by_bypass));
        by_self
            || (has(other.blocking, ty)
                && (other.blocking_bypass == NONE || !tags.contains(&other.blocking_bypass)))
    }
}

/// What an interaction does, by server type.
#[derive(Clone, Debug)]
pub enum Kind {
    Simple,
    Serial(Vec<i32>),
    /// Root interactions: the first is entered, the others forked.
    Parallel(Vec<i32>),
    Condition {
        mode: Option<p::GameMode>,
        /// jumping, swimming, crouching, running, flying, gliding
        states: [Option<bool>; 6],
    },
    Repeat {
        fork: i32,
        repeat: i32,
    },
    Replace {
        var: String,
        default: i32,
    },
    UseBlock,
    PickBlock,
    /// Opens or closes a door (spec 22); horizontal doors (trapdoors)
    /// only open one way.
    Door {
        horizontal: bool,
    },
    UseEntity,
    BreakBlock {
        harvest: bool,
        soft_only: bool,
        tool: Option<String>,
        match_tool: bool,
    },
    PlaceBlock {
        block: i32,
        remove_item: bool,
        quick_replace: bool,
        quick_retype: bool,
        no_physics: bool,
    },
    DragPlace {
        fork: i32,
        per_tick: i32,
        per_gesture: i32,
    },
    PlaceModeSelect([i32; 6]),
    Chaining {
        next: Vec<i32>,
        flags: Vec<i32>,
    },
    Select {
        fail_on: p::FailOnType,
        hit_entity: bool,
        hit_block: bool,
    },
    /// Held input: ends when the client lets go, and goes to the branch
    /// of the longest charge time reached.
    Charging {
        next: Vec<(f32, i32)>,
        failed: i32,
        forks: Vec<(IT, i32)>,
    },
    ModifyInventory(Box<p::ModifyInventoryInteraction>),
    ChangeStat {
        stats: Vec<(i32, f32)>,
        percent: bool,
        behaviour: p::ChangeStatBehaviour,
        /// On the entity that runs the chain (other targets: not yet).
        user: bool,
        ability_cost: bool,
    },
    StatsCondition {
        costs: Vec<(i32, f32)>,
        less_than: bool,
        lenient: bool,
        percent: bool,
        ability_cost: bool,
    },
    /// An entity effect on the entity that runs the chain (other targets:
    /// not yet): applied (for a duration, or the effect's own when 0),
    /// tested, cleared.
    ApplyEffect {
        effect: i32,
        duration: f32,
        user: bool,
    },
    EffectCondition {
        effects: Vec<i32>,
        all: bool,
        user: bool,
    },
    ClearEffect {
        effect: i32,
        user: bool,
    },
    /// Changes the active hotbar slot: to a fixed slot, or to the chain's
    /// target slot.
    ChangeActiveSlot(i32),
    /// Not run on the server yet: the step follows the client's state.
    /// `branches` are the label targets its type compiles, after itself.
    Other {
        branches: Branches,
        /// A block type: the server runs it when the client's data is
        /// there and ends it by its run time, without waiting for the
        /// client's verdict.
        block: bool,
    },
}

/// How a type the server does not run yet lays out its branches.
#[derive(Clone, Debug)]
pub enum Branches {
    /// Next, then failed (label 0).
    Plain,
    /// Keyed branches (labels in order), a jump to the end right after the
    /// operation, the last label being the failed branch.
    Charged(Vec<i32>, i32),
    /// Every label has a branch followed by a jump to the end.
    Each(Vec<i32>),
    /// First click: click, then held (label 0).
    Click(i32, i32),
    /// Damage: failed, next, blocked, then per angle and per target.
    Damage(Vec<i32>),
}

pub struct Inter {
    pub ty: String,
    pub kind: Kind,
    pub wait: W,
    pub run_time: f32,
    pub item_change: p::InteractionItemChangeBehavior,
    pub skip_on_click: Vec<p::GameMode>,
    pub view_distance: f64,
    pub rules: Option<Rules>,
    pub tags: Vec<i32>,
    pub next: i32,
    pub failed: i32,
    pub use_latest: bool,
    /// Whether a chain with this interaction needs the client: always,
    /// never, or when the next or failed interaction does.
    remote: Option<bool>,
}

#[derive(Clone, Debug)]
pub enum Op {
    Run { inter: i32, labels: Vec<u32> },
    Jump(u32),
}

pub struct Root {
    pub id: String,
    pub ops: Vec<Op>,
    pub cooldown: Option<p::InteractionCooldown>,
    pub settings: Vec<(p::GameMode, p::RootInteractionSettings)>,
    pub rules: Rules,
    pub tags: Vec<i32>,
    /// A chain of this root needs the client even when the server starts
    /// it.
    pub needs_remote: bool,
}

/// An item's roots per interaction type and its interaction variables.
#[derive(Default)]
pub struct ItemRoots {
    pub roots: Vec<(IT, i32)>,
    pub vars: HashMap<String, i32>,
}

#[derive(Default)]
pub struct Book {
    pub inters: Vec<Option<Inter>>,
    pub roots: Vec<Option<Root>>,
    pub unarmed: Vec<(IT, i32)>,
    pub items: HashMap<String, ItemRoots>,
    /// Block id → its roots per interaction type.
    pub blocks: HashMap<i32, Vec<(IT, i32)>>,
}

struct Builder {
    ops: Vec<(Option<i32>, Vec<usize>, usize)>,
    labels: Vec<u32>,
}

impl Builder {
    fn unresolved(&mut self) -> usize {
        self.labels.push(u32::MAX);
        self.labels.len() - 1
    }
    fn here(&mut self) -> usize {
        self.labels.push(self.ops.len() as u32);
        self.labels.len() - 1
    }
    fn resolve(&mut self, l: usize) {
        self.labels[l] = self.ops.len() as u32;
    }
    fn run(&mut self, inter: i32, labels: Vec<usize>) {
        self.ops.push((Some(inter), labels, 0));
    }
    fn jump(&mut self, to: usize) {
        self.ops.push((None, Vec::new(), to));
    }
}

macro_rules! base_of {
    ($i:expr, [$($nf:ident),*], [$($o:ident),*]) => {
        match $i {
            $(p::Interaction::$nf(x) => (x.wait_for_data_from, x.run_time, x.on_item_change_behavior,
                &x.settings, &x.rules, &x.tags, x.next, x.failed),)*
            $(p::Interaction::$o(x) => (x.wait_for_data_from, x.run_time, x.on_item_change_behavior,
                &x.settings, &x.rules, &x.tags, NONE, NONE),)*
        }
    };
}

impl Book {
    /// From the registry packets (interactions, root interactions, unarmed
    /// interactions, items, block types) and the server types table.
    pub fn build(
        interactions: &[u8],
        roots: &[u8],
        unarmed: Option<&[u8]>,
        items: Option<&[u8]>,
        blocks: Option<&[u8]>,
        table: &str,
    ) -> anyhow::Result<Book> {
        let mut types: HashMap<i32, (String, serde_json::Value)> = HashMap::new();
        for l in table.lines() {
            let mut f = l.splitn(3, '\t');
            if let (Some(i), Some(t), Some(j)) = (f.next(), f.next(), f.next())
                && let Ok(i) = i.parse()
            {
                types.insert(
                    i,
                    (t.to_owned(), serde_json::from_str(j).unwrap_or_default()),
                );
            }
        }
        let mut book = Book::default();
        let (m, _) = p::UpdateInteractions::decode(interactions, 0)?;
        for (i, x) in m.interactions.into_iter().flatten() {
            let (ty, extra) = types
                .remove(&i)
                .unwrap_or_else(|| ("Simple".into(), serde_json::Value::Null));
            let inter = Self::inter(&x, ty, &extra);
            let i = i as usize;
            if book.inters.len() <= i {
                book.inters.resize_with(i + 1, || None);
            }
            book.inters[i] = Some(inter);
        }
        let (m, _) = p::UpdateRootInteractions::decode(roots, 0)?;
        for (i, r) in m.interactions.into_iter().flatten() {
            let mut b = Builder {
                ops: Vec::new(),
                labels: Vec::new(),
            };
            for &x in &r.interactions {
                book.compile(x, &mut b, 0);
            }
            let ops = b
                .ops
                .iter()
                .map(|(inter, labels, to)| match inter {
                    Some(i) => Op::Run {
                        inter: *i,
                        labels: labels.iter().map(|&l| b.labels[l]).collect(),
                    },
                    None => Op::Jump(b.labels[*to]),
                })
                .collect();
            let mut memo = HashMap::new();
            let needs_remote = r
                .interactions
                .iter()
                .any(|&x| book.needs_remote(x, &mut memo, 0));
            let root = Root {
                needs_remote,
                id: r.id.clone().unwrap_or_default(),
                ops,
                cooldown: r.cooldown.clone(),
                settings: r.settings.clone().unwrap_or_default(),
                rules: Rules::of(&r.rules),
                tags: r.tags.clone().unwrap_or_default(),
            };
            let i = i as usize;
            if book.roots.len() <= i {
                book.roots.resize_with(i + 1, || None);
            }
            book.roots[i] = Some(root);
        }
        if let Some(u) = unarmed {
            let (m, _) = p::UpdateUnarmedInteractions::decode(u, 0)?;
            book.unarmed = m.interactions.unwrap_or_default();
        }
        if let Some(b) = items {
            let (m, _) = p::UpdateItems::decode(b, 0)?;
            for (k, it) in m.items.into_iter().flatten() {
                let r = ItemRoots {
                    roots: it.interactions.clone().unwrap_or_default(),
                    vars: it
                        .interaction_vars
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                };
                if !r.roots.is_empty() || !r.vars.is_empty() {
                    book.items.insert(k, r);
                }
            }
        }
        if let Some(b) = blocks {
            let (m, _) = p::UpdateBlockTypes::decode(b, 0)?;
            for (i, bt) in m.block_types.into_iter().flatten() {
                if let Some(v) = bt.interactions.filter(|v| !v.is_empty()) {
                    book.blocks.insert(i, v);
                }
            }
        }
        Ok(book)
    }

    fn inter(x: &p::Interaction, ty: String, extra: &serde_json::Value) -> Inter {
        let (wait, run_time, item_change, settings, rules, tags, next, failed) = base_of!(
            x,
            [
                SimpleBlockInteraction,
                SimpleInteraction,
                PlaceBlockInteraction,
                BreakBlockInteraction,
                PickBlockInteraction,
                UseBlockInteraction,
                UseEntityInteraction,
                BuilderToolInteraction,
                ModifyInventoryInteraction,
                ConditionInteraction,
                StatsConditionInteraction,
                BlockConditionInteraction,
                ChangeBlockInteraction,
                ChangeStateInteraction,
                SelectInteraction,
                DamageEntityInteraction,
                RepeatInteraction,
                EffectConditionInteraction,
                ApplyForceInteraction,
                ApplyEffectInteraction,
                ClearEntityEffectInteraction,
                ChangeStatInteraction,
                MovementConditionInteraction,
                ProjectileInteraction,
                RemoveEntityInteraction,
                ResetCooldownInteraction,
                TriggerCooldownInteraction,
                CooldownConditionInteraction,
                ChainFlagInteraction,
                IncrementCooldownInteraction,
                CancelChainInteraction,
                RunRootInteraction,
                CameraInteraction,
                SpawnDeployableFromRaycastInteraction,
                ToggleGliderInteraction,
                DurabilityConditionInteraction,
                DragPlaceBlockInteraction,
                ExtrudePlaceBlockInteraction,
                SurfaceDrawPlaceBlockInteraction,
                PlaceModeSelectInteraction,
                DragEraseBlockInteraction,
                GroundedConditionInteraction
            ],
            [
                ChargingInteraction,
                WieldingInteraction,
                ChainingInteraction,
                ReplaceInteraction,
                FirstClickInteraction,
                ParallelInteraction,
                ChangeActiveSlotInteraction,
                SerialInteraction,
                MemoriesConditionInteraction
            ]
        );
        let mut use_latest = false;
        use p::Interaction as I;
        let kind = match x {
            I::SerialInteraction(s) => Kind::Serial(s.serial_interactions.clone()),
            I::ParallelInteraction(s) => Kind::Parallel(s.next.clone()),
            I::ConditionInteraction(s) => Kind::Condition {
                mode: s.required_game_mode,
                states: [
                    s.jumping,
                    s.swimming,
                    s.crouching,
                    s.running,
                    s.flying,
                    s.gliding,
                ],
            },
            I::RepeatInteraction(s) => Kind::Repeat {
                fork: s.fork_interactions,
                repeat: s.repeat,
            },
            I::ReplaceInteraction(s) => Kind::Replace {
                var: s.variable.clone().unwrap_or_default(),
                default: s.default_value,
            },
            I::UseBlockInteraction(s) if ty == "UseBlock" => {
                use_latest = s.use_latest_target;
                Kind::UseBlock
            }
            I::PickBlockInteraction(s) if ty == "PickBlock" => {
                use_latest = s.use_latest_target;
                Kind::PickBlock
            }
            I::UseEntityInteraction(_) if ty == "UseEntity" => Kind::UseEntity,
            I::BreakBlockInteraction(s) if ty == "BreakBlock" => {
                use_latest = s.use_latest_target;
                Kind::BreakBlock {
                    harvest: s.harvest,
                    soft_only: s.soft_only,
                    tool: extra
                        .get("Tool")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    match_tool: extra
                        .get("MatchTool")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                }
            }
            I::PlaceBlockInteraction(s) if ty == "PlaceBlock" => Kind::PlaceBlock {
                block: s.block_id,
                remove_item: s.remove_item_in_hand,
                quick_replace: s.quick_replace,
                quick_retype: s.quick_retype,
                no_physics: s.no_physics,
            },
            I::DragPlaceBlockInteraction(s) if ty == "DragPlaceBlock" => Kind::DragPlace {
                fork: s.fork_interactions,
                per_tick: s.max_blocks_per_tick,
                per_gesture: s.max_blocks_per_gesture,
            },
            I::PlaceModeSelectInteraction(s) => Kind::PlaceModeSelect([
                s.default_interactions,
                s.replace_interactions,
                s.retype_interactions,
                s.extrude_interactions,
                s.surface_draw_interactions,
                s.fast_place_interactions,
            ]),
            I::ChainingInteraction(s) => {
                let mut flags = s.flags.clone().unwrap_or_default();
                flags.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
                Kind::Chaining {
                    next: s.chaining_next.clone(),
                    flags: flags.into_iter().map(|f| f.1).collect(),
                }
            }
            I::SelectInteraction(s) => Kind::Select {
                fail_on: s.fail_on,
                hit_entity: s.hit_entity != NONE || s.hit_entity_rules.is_some(),
                hit_block: extra.get("HitBlock").is_some(),
            },
            I::ChargingInteraction(s) => {
                let mut next = s.charged_next.clone().unwrap_or_default();
                next.sort_by(|a, b| a.0.total_cmp(&b.0));
                Kind::Charging {
                    next,
                    failed: s.failed,
                    forks: s.forks.clone().unwrap_or_default(),
                }
            }
            I::ModifyInventoryInteraction(s) => Kind::ModifyInventory(s.clone()),
            I::ApplyEffectInteraction(s) if ty == "ApplyEffect" => Kind::ApplyEffect {
                effect: s.effect_id,
                duration: extra
                    .get("Duration")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32,
                user: s.entity_target == p::InteractionTarget::USER,
            },
            I::EffectConditionInteraction(s) if ty == "EffectCondition" => Kind::EffectCondition {
                effects: s.entity_effects.clone(),
                all: s.r#match == p::Match::ALL,
                user: s.entity_target == p::InteractionTarget::USER,
            },
            I::ClearEntityEffectInteraction(s) if ty == "ClearEntityEffect" => Kind::ClearEffect {
                effect: s.effect_id,
                user: s.entity_target == p::InteractionTarget::USER,
            },
            I::ChangeStatInteraction(s) if ty == "ChangeStat" => Kind::ChangeStat {
                stats: s.stat_modifiers.clone().unwrap_or_default(),
                percent: s.value_type == p::ValueType::PERCENT,
                behaviour: s.change_stat_behaviour,
                user: s.entity_target == p::InteractionTarget::USER,
                ability_cost: s.use_ability_cost,
            },
            I::StatsConditionInteraction(s) if ty == "StatsCondition" => Kind::StatsCondition {
                costs: s.costs.clone().unwrap_or_default(),
                less_than: s.less_than,
                lenient: s.lenient,
                percent: s.value_type == p::ValueType::PERCENT,
                ability_cost: s.use_ability_cost,
            },
            I::WieldingInteraction(s) => Kind::Other {
                branches: Self::charged(&s.charged_next, s.failed),
                block: false,
            },
            I::MemoriesConditionInteraction(s) => {
                let mut v = s.memories_next.clone();
                v.sort_by_key(|e| e.0);
                Kind::Other {
                    branches: Branches::Charged(v.into_iter().map(|e| e.1).collect(), s.failed),
                    block: false,
                }
            }
            I::FirstClickInteraction(s) => Kind::Other {
                branches: Branches::Click(s.click, s.held),
                block: false,
            },
            I::ApplyForceInteraction(s) => {
                let or = |x: i32| if x == NONE { s.next } else { x };
                Kind::Other {
                    branches: Branches::Each(vec![s.next, or(s.ground_next), or(s.collision_next)]),
                    block: false,
                }
            }
            I::MovementConditionInteraction(s) => Kind::Other {
                branches: Branches::Each(vec![
                    s.failed,
                    s.forward,
                    s.back,
                    s.left,
                    s.right,
                    s.forward_left,
                    s.forward_right,
                    s.back_left,
                    s.back_right,
                ]),
                block: false,
            },
            I::DamageEntityInteraction(s) => {
                let or = |x: i32| if x == NONE { s.next } else { x };
                let mut v = vec![s.failed, s.next, s.blocked];
                v.extend(s.angled_damage.iter().flatten().map(|a| or(a.next)));
                let mut t: Vec<_> = s.targeted_damage.iter().collect();
                t.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
                v.extend(t.into_iter().map(|a| or(a.1.next)));
                Kind::Other {
                    branches: Branches::Damage(v),
                    block: false,
                }
            }
            I::ChangeActiveSlotInteraction(s) => Kind::ChangeActiveSlot(s.target_slot),
            _ if ty == "Simple" => Kind::Simple,
            I::SimpleBlockInteraction(s) if ty == "Door" => {
                use_latest = s.use_latest_target;
                Kind::Door {
                    horizontal: extra.get("Horizontal").and_then(|v| v.as_bool()) == Some(true),
                }
            }
            I::SimpleBlockInteraction(s) => {
                use_latest = s.use_latest_target;
                Kind::Other {
                    branches: Branches::Plain,
                    block: true,
                }
            }
            I::ChangeBlockInteraction(_)
            | I::ChangeStateInteraction(_)
            | I::BlockConditionInteraction(_)
            | I::BreakBlockInteraction(_)
            | I::PickBlockInteraction(_)
            | I::UseBlockInteraction(_) => {
                use_latest = match x {
                    I::ChangeBlockInteraction(s) => s.use_latest_target,
                    I::ChangeStateInteraction(s) => s.use_latest_target,
                    I::BlockConditionInteraction(s) => s.use_latest_target,
                    I::BreakBlockInteraction(s) => s.use_latest_target,
                    I::PickBlockInteraction(s) => s.use_latest_target,
                    I::UseBlockInteraction(s) => s.use_latest_target,
                    _ => false,
                };
                Kind::Other {
                    branches: Branches::Plain,
                    block: true,
                }
            }
            _ => Kind::Other {
                branches: Branches::Plain,
                block: false,
            },
        };
        let remote = match x {
            I::ApplyForceInteraction(_)
            | I::BuilderToolInteraction(_)
            | I::CameraInteraction(_)
            | I::PlaceBlockInteraction(_)
            | I::ChainingInteraction(_)
            | I::ChangeActiveSlotInteraction(_)
            | I::ChargingInteraction(_)
            | I::WieldingInteraction(_)
            | I::DamageEntityInteraction(_)
            | I::DragPlaceBlockInteraction(_)
            | I::ExtrudePlaceBlockInteraction(_)
            | I::SurfaceDrawPlaceBlockInteraction(_)
            | I::DragEraseBlockInteraction(_)
            | I::FirstClickInteraction(_)
            | I::MovementConditionInteraction(_)
            | I::ParallelInteraction(_)
            | I::PickBlockInteraction(_)
            | I::PlaceModeSelectInteraction(_)
            | I::ProjectileInteraction(_)
            | I::RepeatInteraction(_)
            | I::ReplaceInteraction(_)
            | I::SelectInteraction(_)
            | I::SimpleBlockInteraction(_)
            | I::BreakBlockInteraction(_)
            | I::UseBlockInteraction(_)
            | I::ChangeBlockInteraction(_)
            | I::ChangeStateInteraction(_)
            | I::BlockConditionInteraction(_)
            | I::SpawnDeployableFromRaycastInteraction(_)
            | I::UseEntityInteraction(_) => Some(true),
            I::SerialInteraction(_) | I::MemoriesConditionInteraction(_) => Some(false),
            _ => match ty.as_str() {
                "RunOnBlockTypes" => Some(true),
                "SignalNearbyVolumes"
                | "SpawnDeployableAtHitLocation"
                | "SpawnDeployableAtLocation"
                | "SpawnTriggerVolume" => Some(false),
                _ => None,
            },
        };
        Inter {
            ty,
            kind,
            wait,
            run_time,
            item_change,
            skip_on_click: settings
                .iter()
                .flatten()
                .filter(|s| s.1.allow_skip_on_click)
                .map(|s| s.0)
                .collect(),
            view_distance: extra
                .get("ViewDistance")
                .and_then(|v| v.as_f64())
                .unwrap_or(96.0),
            rules: Some(Rules::of(rules)),
            tags: tags.clone().unwrap_or_default(),
            next,
            failed,
            use_latest,
            remote,
        }
    }

    fn charged(next: &Option<Vec<(f32, i32)>>, failed: i32) -> Branches {
        let mut v = next.clone().unwrap_or_default();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        Branches::Charged(v.into_iter().map(|e| e.1).collect(), failed)
    }

    fn needs_remote(&self, i: i32, memo: &mut HashMap<i32, bool>, depth: u32) -> bool {
        let Some(x) = self.inter_at(i) else {
            return false;
        };
        if let Some(r) = x.remote {
            return r;
        }
        if let Some(&r) = memo.get(&i) {
            return r;
        }
        let r = depth < 512
            && (self.needs_remote(x.next, memo, depth + 1)
                || self.needs_remote(x.failed, memo, depth + 1));
        memo.insert(i, r);
        r
    }

    pub fn inter_at(&self, i: i32) -> Option<&Inter> {
        self.inters.get(usize::try_from(i).ok()?)?.as_ref()
    }

    pub fn root(&self, i: i32) -> Option<&Root> {
        self.roots.get(usize::try_from(i).ok()?)?.as_ref()
    }

    /// The plain layout: the operation, its next branch, a jump over the
    /// failed branch when there is one, the failed branch.
    fn plain(&self, i: i32, next: i32, failed: i32, b: &mut Builder, depth: u32) {
        if next == NONE && failed == NONE {
            return b.run(i, Vec::new());
        }
        let (failed_label, end) = (b.unresolved(), b.unresolved());
        b.run(i, vec![failed_label]);
        if next != NONE {
            self.compile(next, b, depth);
        }
        if failed != NONE {
            b.jump(end);
        }
        b.resolve(failed_label);
        if failed != NONE {
            self.compile(failed, b, depth);
        }
        b.resolve(end);
    }

    fn compile(&self, i: i32, b: &mut Builder, depth: u32) {
        let Some(x) = self.inter_at(i) else { return };
        if depth > 512 {
            return;
        }
        let depth = depth + 1;
        match &x.kind {
            Kind::Serial(list) => {
                for &c in list {
                    self.compile(c, b, depth);
                }
            }
            Kind::Parallel(_) | Kind::Replace { .. } | Kind::ChangeActiveSlot(_) => {
                b.run(i, Vec::new())
            }
            Kind::Chaining { next, flags, .. } => {
                let labels: Vec<usize> = (0..next.len() + flags.len())
                    .map(|_| b.unresolved())
                    .collect();
                b.run(i, labels.clone());
                let end = b.unresolved();
                for (n, &c) in next.iter().chain(flags.iter()).enumerate() {
                    b.resolve(labels[n]);
                    self.compile(c, b, depth);
                    b.jump(end);
                }
                b.resolve(end);
            }
            Kind::PlaceModeSelect(branches) => {
                let mut labels = vec![0usize; 6];
                let at = b.ops.len();
                b.run(i, Vec::new());
                let end = b.unresolved();
                labels[0] = b.here();
                self.compile(branches[0], b, depth);
                b.jump(end);
                for n in 1..6 {
                    if branches[n] == NONE {
                        labels[n] = labels[0];
                    } else {
                        labels[n] = b.here();
                        self.compile(branches[n], b, depth);
                        b.jump(end);
                    }
                }
                b.resolve(end);
                b.ops[at].1 = labels;
                if x.next != NONE {
                    self.compile(x.next, b, depth);
                }
            }
            Kind::Charging { next, failed, .. } => {
                let end = b.unresolved();
                let labels: Vec<usize> = (0..next.len() + 1).map(|_| b.unresolved()).collect();
                b.run(i, labels.clone());
                b.jump(end);
                for (n, c) in next.iter().enumerate() {
                    b.resolve(labels[n]);
                    self.compile(c.1, b, depth);
                    b.jump(end);
                }
                b.resolve(labels[next.len()]);
                if *failed != NONE {
                    self.compile(*failed, b, depth);
                }
                b.resolve(end);
            }
            Kind::Other { branches, .. } => match branches {
                Branches::Plain => self.plain(i, x.next, x.failed, b, depth),
                Branches::Click(click, held) => self.plain(i, *click, *held, b, depth),
                Branches::Charged(keyed, failed) => {
                    let end = b.unresolved();
                    let labels: Vec<usize> = (0..keyed.len() + 1).map(|_| b.unresolved()).collect();
                    b.run(i, labels.clone());
                    b.jump(end);
                    for (n, &c) in keyed.iter().enumerate() {
                        b.resolve(labels[n]);
                        self.compile(c, b, depth);
                        b.jump(end);
                    }
                    b.resolve(labels[keyed.len()]);
                    if *failed != NONE {
                        self.compile(*failed, b, depth);
                    }
                    b.resolve(end);
                }
                Branches::Each(list) => {
                    let labels: Vec<usize> = (0..list.len()).map(|_| b.unresolved()).collect();
                    b.run(i, labels.clone());
                    let end = b.unresolved();
                    for (n, &c) in list.iter().enumerate() {
                        b.resolve(labels[n]);
                        if c != NONE {
                            self.compile(c, b, depth);
                        }
                        b.jump(end);
                    }
                    b.resolve(end);
                }
                Branches::Damage(list) => {
                    let at = b.ops.len();
                    b.run(i, Vec::new());
                    let end = b.unresolved();
                    let mut labels = Vec::new();
                    for &c in list {
                        labels.push(b.here());
                        if c != NONE {
                            self.compile(c, b, depth);
                        }
                        b.jump(end);
                    }
                    b.resolve(end);
                    b.ops[at].1 = labels;
                }
            },
            _ => self.plain(i, x.next, x.failed, b, depth),
        }
    }

    /// The root an item (or an empty hand) gives an interaction type.
    pub fn held_root(&self, item: Option<&str>, ty: IT) -> Option<i32> {
        let list = match item {
            Some(k) => &self.items.get(k)?.roots,
            None => &self.unarmed,
        };
        list.iter().find(|e| e.0 == ty).map(|e| e.1)
    }

    pub fn block_root(&self, block: i32, ty: IT) -> Option<i32> {
        self.blocks
            .get(&block)?
            .iter()
            .find(|e| e.0 == ty)
            .map(|e| e.1)
    }
}

/// What a chain runs for: the held item it started with and its targets.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    /// The inventory section of the held item (−1 hotbar, −5 utility, −3
    /// armor, −8 tools).
    pub section: i32,
    pub slot: i32,
    /// The item the chain started with (its interaction variables).
    pub original: Option<String>,
    /// The item the chain holds now.
    pub held: Option<String>,
    pub target_block: Option<Pos>,
    pub target_raw: Option<Pos>,
    pub target_entity: Option<i32>,
    pub target_slot: Option<i32>,
    pub hit_location: Option<[f32; 3]>,
    pub hit_detail: Option<String>,
    /// An inventory move that waits for the chain's slot change.
    pub moved: Option<Move>,
}

/// An item move between two slots: (section, slot), a quantity, (section,
/// slot).
#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub from: (i32, i32),
    pub quantity: i32,
    pub to: (i32, i32),
}

/// A placement the client reports for a place operation. The face, the
/// operation's own block and its quick-replace, quick-retype and
/// no-physics settings are carried for the placing rules that use them.
#[allow(dead_code)]
pub struct PlaceRequest {
    pub pos: Pos,
    pub rotation: p::BlockRotation,
    pub face: p::BlockFace,
    pub client_block: i32,
    pub block: i32,
    pub remove_item: bool,
    pub quick_replace: bool,
    pub quick_retype: bool,
    pub no_physics: bool,
    pub slot: i32,
}

/// What running operations needs from the world, for one entity.
pub trait Host {
    fn creative(&self) -> bool;
    /// The server still waits for the client to be ready: no timeouts.
    fn waiting_for_client(&self) -> bool {
        false
    }
    /// How long to wait for the client's side of a step, in ms.
    fn timeout_ms(&self) -> i64 {
        3000
    }
    /// The active slot of a section, −1 without one.
    fn active_slot(&self, section: i32) -> i32;
    /// The item in a slot.
    fn item_at(&self, section: i32, slot: i32) -> Option<String>;
    fn movement(&self) -> p::MovementStates;
    /// The label a place mode select takes: 0, or the creative place mode.
    fn place_mode(&self) -> usize {
        0
    }
    /// The block id of a cell, `None` when its section is not loaded.
    fn block(&self, pos: Pos) -> Option<i32>;
    /// A cell's block id and rotation, for a step's state.
    fn cell(&self, pos: Pos) -> Option<(i32, p::BlockRotation)>;
    /// The base cell of a block that fills several.
    fn base_of(&self, pos: Pos) -> Pos;
    fn in_reach(&self, held: Option<&str>, pos: Pos) -> bool;
    /// Places a block (spec 15); `None` when refused before placing. The
    /// second value tells the held stack's last unit was used.
    fn place(&mut self, r: &PlaceRequest) -> Option<bool>;
    /// One break operation on a block (spec 12); false when not allowed.
    fn break_block(
        &mut self,
        pos: Pos,
        ctx: &Ctx,
        harvest: bool,
        soft_only: bool,
        tool: Option<&str>,
        match_tool: bool,
    ) -> bool;
    /// A door is used: it opens or closes; false when it is blocked.
    fn door(&mut self, pos: Pos, horizontal: bool) -> bool;
    /// A type the server does not run itself starts a step.
    fn other(&mut self, ty: &str, ctx: &Ctx);
    fn cancel(&mut self, chain: i32, fork: Option<p::ForkedChainId>);
    fn set_active_slot(&mut self, section: i32, slot: i32);
    /// The entity's active hotbar slot changes.
    fn change_hotbar_slot(&mut self, slot: i32);
    /// An interaction changes the inventory (items removed and added, the
    /// held stack's quantity and durability); false when it cannot.
    fn modify_inventory(&mut self, m: &p::ModifyInventoryInteraction, ctx: &mut Ctx) -> bool;
    /// Stat changes of an interaction, predicted by the entity's client.
    fn change_stats(
        &mut self,
        stats: &[(i32, f32)],
        percent: bool,
        behaviour: p::ChangeStatBehaviour,
    );
    /// An effect is put on the entity (`None`: for the effect's own time).
    fn apply_effect(&mut self, effect: i32, duration: Option<f32>);
    fn has_effect(&self, effect: i32) -> bool;
    fn clear_effect(&mut self, effect: i32);
    /// A stat's value, minimum and maximum.
    fn stat(&self, index: i32) -> Option<(f32, f32, f32)>;
    /// The inventory move a slot change was waiting for is made now, and
    /// the whole inventory sent.
    fn move_item(&mut self, m: &Move);
    /// Tells the players around about a step.
    fn play(&mut self, packet: p::PlayInteractionFor, view_distance: f64, owner_too: bool);
    fn network_id(&self) -> i32;
}

#[derive(Default)]
struct Cooldown {
    max: f32,
    charges: Vec<f32>,
    remaining: f32,
    timer: f32,
    count: usize,
    interrupt: bool,
}

impl Cooldown {
    fn full(&self) -> bool {
        self.count >= self.charges.len()
    }
    /// Whether it is on cooldown; taking a charge when it is not.
    fn check(&mut self) -> bool {
        if self.remaining <= 0.0 && self.count > 0 {
            self.count -= 1;
            if self.interrupt {
                self.timer = 0.0;
            }
            self.remaining = self.max;
            false
        } else {
            true
        }
    }
    /// True when it can be forgotten.
    fn tick(&mut self, dt: f32) -> bool {
        if !self.full() {
            self.timer += dt;
            if self.timer >= self.charges[self.count] {
                self.count += 1;
                self.timer = 0.0;
            }
        }
        self.remaining -= dt;
        (self.full() || self.charges.len() <= 1) && self.remaining <= 0.0
    }
}

struct Entry {
    index: i32,
    timestamp: i64,
    server: Data,
    client: Option<p::InteractionSyncData>,
    waiting_sync: i64,
    waiting_server: i64,
    desynced: bool,
    send_initial: bool,
    shift: Option<f32>,
    forked: i32,
    repeats: i32,
    fork: Option<(i32, i32)>,
    /// Forks made per interaction type (charging).
    counts: Vec<(IT, i32)>,
}

impl Entry {
    fn time(&self, now: i64) -> f32 {
        if self.timestamp == 0 {
            0.0
        } else {
            (now - self.timestamp) as f32 / 1.0e9
        }
    }
    /// False when the client's state is for another operation.
    fn set_client(&mut self, c: Option<p::InteractionSyncData>) -> bool {
        if let Some(c) = &c {
            let s = self.server.lock().unwrap();
            if c.operation_counter != s.operation_counter
                || c.root_interaction != s.root_interaction
            {
                return false;
            }
        }
        self.client = c;
        true
    }
    fn state(&self) -> St {
        self.server.lock().unwrap().state
    }
}

/// Client data that arrived for a fork the server has not made yet.
#[derive(Default)]
struct Temp {
    client_state: Option<St>,
    sync: Vec<Option<p::InteractionSyncData>>,
    forks: HashMap<(i32, i32), Temp>,
    fork_id: Option<p::ForkedChainId>,
    root: i32,
    data: Option<p::InteractionChainData>,
}

pub struct Chain {
    ty: IT,
    base_ty: IT,
    id: i32,
    fork_id: Option<p::ForkedChainId>,
    base_fork: Option<(i32, i32)>,
    data: p::InteractionChainData,
    pub ctx: Ctx,
    forks: Vec<((i32, i32), Chain)>,
    temp: HashMap<(i32, i32), Temp>,
    new_forks: Vec<(i32, i32)>,
    initial_root: i32,
    root: i32,
    counter: usize,
    call_stack: Vec<(i32, usize)>,
    op_index: i32,
    index_offset: i32,
    entries: Vec<Entry>,
    temp_sync: Vec<Option<p::InteractionSyncData>>,
    temp_sync_offset: i32,
    waiting_server: i64,
    waiting_client: i64,
    client_state: St,
    server_state: St,
    final_state: St,
    sent_initial: bool,
    desynced: bool,
    time_shift: f32,
    first_run: bool,
    is_first_run: bool,
    pre_ticked: bool,
    skip_on_click: bool,
    predicted: bool,
    completed: bool,
    /// The client runs this chain too.
    requires_client: bool,
}

/// One chain's states of a tick, as they go to the client.
struct Sync {
    initial_root: Option<i32>,
    ty: IT,
    equip_slot: i32,
    id: i32,
    fork_id: Option<p::ForkedChainId>,
    data: p::InteractionChainData,
    state: St,
    forks: Vec<Sync>,
    base: i32,
    states: Option<Vec<Option<Data>>>,
}

impl Sync {
    fn packet(&self) -> p::SyncInteractionChain {
        p::SyncInteractionChain {
            initial: self.initial_root.is_some(),
            override_root_interaction: self.initial_root.unwrap_or(NONE),
            interaction_type: self.ty,
            equip_slot: self.equip_slot,
            chain_id: self.id,
            forked_id: self.fork_id.clone(),
            data: Some(self.data.clone()),
            state: self.state,
            new_forks: (!self.forks.is_empty())
                .then(|| self.forks.iter().map(Sync::packet).collect()),
            operation_base_index: self.base,
            interaction_data: self.states.as_ref().map(|v| {
                v.iter()
                    .map(|d| d.as_ref().map(|d| d.lock().unwrap().clone()))
                    .collect()
            }),
            ..Default::default()
        }
    }
}

struct Cancelled(St);

fn fork_key(f: &p::ForkedChainId) -> (i32, i32) {
    (f.entry_index, f.sub_index)
}

impl Chain {
    fn new(ty: IT, ctx: Ctx, data: p::InteractionChainData, root: i32) -> Chain {
        Chain {
            ty,
            base_ty: ty,
            id: 0,
            fork_id: None,
            base_fork: None,
            data,
            ctx,
            forks: Vec::new(),
            temp: HashMap::new(),
            new_forks: Vec::new(),
            initial_root: root,
            root,
            counter: 0,
            call_stack: Vec::new(),
            op_index: 0,
            index_offset: 0,
            entries: Vec::new(),
            temp_sync: Vec::new(),
            temp_sync_offset: 0,
            waiting_server: 0,
            waiting_client: 0,
            client_state: St::NOT_FINISHED,
            server_state: St::NOT_FINISHED,
            final_state: St::FINISHED,
            sent_initial: false,
            desynced: false,
            time_shift: 0.0,
            first_run: true,
            is_first_run: true,
            pre_ticked: false,
            skip_on_click: false,
            predicted: false,
            completed: false,
            requires_client: true,
        }
    }

    fn fork_mut(&mut self, key: (i32, i32)) -> Option<&mut Chain> {
        self.forks.iter_mut().find(|f| f.0 == key).map(|f| &mut f.1)
    }

    fn entry_pos(&self, index: i32) -> Option<usize> {
        let o = index - self.index_offset;
        (o >= 0 && (o as usize) < self.entries.len()).then_some(o as usize)
    }

    /// The entry of an operation index, made when the chain reaches it.
    fn entry_or_new(&mut self, index: i32) -> usize {
        let o = (index - self.index_offset).max(0) as usize;
        if o >= self.entries.len() {
            self.entries.push(Entry {
                index,
                timestamp: 0,
                server: Arc::new(Mutex::new(data(self.counter as i32, self.root))),
                client: None,
                waiting_sync: 0,
                waiting_server: 0,
                desynced: false,
                send_initial: true,
                shift: None,
                forked: 0,
                repeats: 0,
                fork: None,
                counts: Vec::new(),
            });
            return self.entries.len() - 1;
        }
        o
    }

    fn flag_desync(&mut self) {
        self.desynced = true;
        for f in &mut self.forks {
            f.1.flag_desync();
        }
    }

    fn put_sync(&mut self, index: i32, d: p::InteractionSyncData) {
        let i = index - self.temp_sync_offset;
        if i < 0 {
            return;
        }
        let i = i as usize;
        if i < self.temp_sync.len() {
            self.temp_sync[i] = Some(d);
        } else if i == self.temp_sync.len() {
            self.temp_sync.push(Some(d));
        }
    }

    /// Drops the client states kept from an operation index on, and the
    /// ones already attached to entries from there.
    fn clear_sync(&mut self, index: i32) {
        let t = (index - self.temp_sync_offset).max(0) as usize;
        self.temp_sync.truncate(t);
        let i = (index - self.index_offset).max(0) as usize;
        for e in self.entries.iter_mut().skip(i) {
            e.client = None;
        }
    }

    fn take_sync(&mut self, index: i32) -> Option<p::InteractionSyncData> {
        if index != self.temp_sync_offset || self.temp_sync.first()?.is_none() {
            return None;
        }
        self.temp_sync_offset += 1;
        self.temp_sync.remove(0)
    }

    fn pop_root(&mut self) {
        if let Some((root, counter)) = self.call_stack.pop() {
            self.root = root;
            self.counter = counter + 1;
        }
    }

    fn update_server_state(&mut self, book: &Book) {
        if self.server_state != St::NOT_FINISHED {
            return;
        }
        let max = book.root(self.root).map_or(0, |r| r.ops.len());
        if self.counter >= max {
            self.server_state = self.final_state;
        } else {
            let e = self.entry_or_new(self.op_index);
            self.server_state = match self.entries[e].state() {
                St::NOT_FINISHED | St::FINISHED => St::NOT_FINISHED,
                _ => St::FAILED,
            };
        }
    }

    /// The fork id of a new fork made by the entry at `e`.
    fn next_fork_id(&mut self, e: usize) -> (p::ForkedChainId, (i32, i32)) {
        let key = {
            let mut s = self.entries[e].server.lock().unwrap();
            s.total_forks += 1;
            (self.entries[e].index, s.total_forks - 1)
        };
        let new = p::ForkedChainId {
            entry_index: key.0,
            sub_index: key.1,
            forked_id: None,
        };
        let full = match &self.fork_id {
            Some(f) => {
                let mut f = f.clone();
                let mut tail = &mut f;
                while tail.forked_id.is_some() {
                    tail = tail.forked_id.as_mut().unwrap();
                }
                tail.forked_id = Some(Box::new(new));
                f
            }
            None => new,
        };
        (full, key)
    }
}

fn sync_temp(t: &mut Temp, packet: &p::SyncInteractionChain) {
    if packet.initial && packet.override_root_interaction != NONE {
        t.root = packet.override_root_interaction;
    }
    for f in packet.new_forks.iter().flatten() {
        if let (Some(id), Some(_)) = (&f.forked_id, &f.data) {
            let mut base = id;
            while let Some(n) = &base.forked_id {
                base = n;
            }
            let sub = t.forks.entry(fork_key(base)).or_default();
            sub.fork_id = f.forked_id.clone();
            sub.data = f.data.clone();
            sync_temp(sub, f);
        }
    }
    let Some(states) = &packet.interaction_data else {
        t.client_state = Some(packet.state);
        return;
    };
    if packet.operation_base_index < 0 {
        return;
    }
    for (i, d) in states.iter().enumerate() {
        let Some(d) = d else { continue };
        let index = packet.operation_base_index as usize + i;
        if index < t.sync.len() {
            t.sync[index] = Some(d.clone());
        } else if index == t.sync.len() {
            t.sync.push(Some(d.clone()));
        }
    }
    t.sync
        .truncate(packet.operation_base_index as usize + states.len());
    t.client_state = Some(packet.state);
}

/// Where a packet's chain is.
enum Found<'a, 'b> {
    Chain(&'a mut Chain),
    /// The fork named from here on does not exist (yet) under this chain.
    Missing(&'a mut Chain, &'b p::ForkedChainId),
    None,
}

fn find<'a, 'b>(
    chains: &'a mut [Chain],
    id: i32,
    fork: Option<&'b p::ForkedChainId>,
) -> Found<'a, 'b> {
    let Some(mut chain) = chains.iter_mut().find(|c| c.id == id) else {
        return Found::None;
    };
    let mut f = fork;
    while let Some(x) = f {
        if chain.forks.iter().any(|e| e.0 == fork_key(x)) {
            chain = chain.fork_mut(fork_key(x)).unwrap();
            f = x.forked_id.as_deref();
        } else {
            return Found::Missing(chain, x);
        }
    }
    Found::Chain(chain)
}

/// One entity's chains.
pub struct Manager {
    chains: Vec<Chain>,
    queue: VecDeque<p::SyncInteractionChain>,
    queue_time: i64,
    last_client: i32,
    now: i64,
    cooldowns: HashMap<String, Cooldown>,
    shift: [f32; TYPES],
    shift_dirty: [bool; TYPES],
    shifts_dirty: bool,
    out: Vec<Sync>,
    last_server: i32,
    /// Chains the server starts, run at the next tick.
    starting: Vec<Chain>,
}

impl Default for Manager {
    fn default() -> Self {
        Manager {
            chains: Vec::new(),
            queue: VecDeque::new(),
            queue_time: 0,
            last_client: 0,
            now: 1,
            cooldowns: HashMap::new(),
            shift: [0.0; TYPES],
            shift_dirty: [false; TYPES],
            shifts_dirty: false,
            out: Vec::new(),
            last_server: 0,
            starting: Vec::new(),
        }
    }
}

impl Manager {
    /// The client's chain packets: queued for the next tick.
    pub fn receive(&mut self, updates: Vec<p::SyncInteractionChain>, host: &mut dyn Host) {
        for u in updates {
            if self.queue.len() < 1000 {
                self.queue.push_back(u);
            } else if u.initial {
                host.cancel(u.chain_id, u.forked_id);
            }
        }
    }

    pub fn is_idle(&self) -> bool {
        self.chains.is_empty()
            && self.queue.is_empty()
            && self.cooldowns.is_empty()
            && self.starting.is_empty()
    }

    /// The server starts a chain of its own; it runs from the next tick,
    /// with a negative id.
    pub fn start_server(
        &mut self,
        book: &Book,
        ty: IT,
        ctx: Ctx,
        root: i32,
        data: p::InteractionChainData,
    ) {
        let mut chain = Chain::new(ty, ctx, data, root);
        chain.requires_client = book.root(root).is_some_and(|r| r.needs_remote);
        self.starting.push(chain);
    }

    /// The states of this tick for the client, if any.
    pub fn take_packet(&mut self) -> Option<p::SyncInteractionChains> {
        if self.out.is_empty() {
            return None;
        }
        self.out.truncate(128);
        let updates = self.out.iter().map(Sync::packet).collect();
        self.out.clear();
        Some(p::SyncInteractionChains { updates })
    }

    pub fn tick(&mut self, book: &Book, host: &mut dyn Host, dt: f32) {
        self.now += TICK_NANOS;
        if self.shifts_dirty {
            let mut clear = true;
            for i in 0..TYPES {
                if !self.shift_dirty[i] {
                    self.shift[i] = 0.0;
                } else {
                    clear = false;
                    self.shift[i] += dt;
                }
            }
            self.shift_dirty = [false; TYPES];
            if clear {
                self.shifts_dirty = false;
            }
        }
        self.cooldowns.retain(|_, c| !c.tick(dt));
        for mut chain in std::mem::take(&mut self.starting) {
            let Some(root) = book.root(chain.initial_root) else {
                continue;
            };
            if self.on_cooldown(root, chain.ty, host, false) {
                continue;
            }
            self.last_server -= 1;
            if self.last_server >= 0 {
                self.last_server = -1;
            }
            chain.id = self.last_server;
            if !self.tick_chain(book, host, &mut chain, None) {
                chain.pre_ticked = true;
                self.chains.push(chain);
            }
        }
        if self.queue.is_empty() {
            self.tick_chains(book, host);
            return;
        }
        let mut first = true;
        while self.consume_queue(book, host) || first {
            self.tick_chains(book, host);
            let shift = self.shift.iter().fold(0.0f32, |a, &b| a.max(b));
            if shift > 0.0 {
                self.cooldowns.retain(|_, c| !c.tick(shift));
            }
            first = false;
        }
    }

    fn tick_chains(&mut self, book: &Book, host: &mut dyn Host) {
        if self.chains.is_empty() {
            return;
        }
        let order = crate::changes::map_order(self.chains.iter().map(|c| c.id));
        let mut chains = std::mem::take(&mut self.chains);
        chains.sort_by_key(|c| order.iter().position(|&i| i == c.id));
        chains.retain_mut(|c| !self.tick_chain(book, host, c, None));
        chains.append(&mut self.chains);
        self.chains = chains;
    }

    fn consume_queue(&mut self, book: &Book, host: &mut dyn Host) -> bool {
        let (mut finished, mut desynced, mut changed) = (false, false, false);
        let mut i = 0;
        while i < self.queue.len() {
            let packet = self.queue[i].clone();
            if packet.desync {
                desynced = true;
            }
            let now = self.now;
            let exists = match find(&mut self.chains, packet.chain_id, packet.forked_id.as_ref()) {
                Found::Chain(_) => true,
                Found::None => false,
                Found::Missing(chain, id) => {
                    // The fork may still come: keep the data for it, unless
                    // the step it names already made that fork or is gone.
                    let gone = match chain.entry_pos(id.entry_index) {
                        Some(e) => {
                            id.sub_index < chain.entries[e].server.lock().unwrap().total_forks
                        }
                        None => id.entry_index < chain.index_offset,
                    };
                    if gone {
                        i += 1;
                    } else {
                        let mut t = chain.temp.entry(fork_key(id)).or_default();
                        let mut next = id.forked_id.as_deref();
                        while let Some(n) = next {
                            t = t.forks.entry(fork_key(n)).or_default();
                            next = n.forked_id.as_deref();
                        }
                        t.fork_id = packet.forked_id.clone();
                        t.data = packet.data.clone();
                        sync_temp(t, &packet);
                        changed = true;
                        self.queue.remove(i);
                        self.queue_time = 0;
                    }
                    continue;
                }
            };
            if !exists && !finished {
                if self.start(book, host, &packet) {
                    changed = true;
                    self.queue.remove(i);
                    self.queue_time = 0;
                    continue;
                }
                let mut dropped = false;
                if !host.waiting_for_client() {
                    let queued = if self.queue_time == 0 {
                        self.queue_time = now;
                        0
                    } else {
                        now - self.queue_time
                    };
                    if queued > host.timeout_ms() * 1_000_000 {
                        host.cancel(packet.chain_id, packet.forked_id.clone());
                        self.queue.remove(i);
                        dropped = true;
                    }
                }
                if !desynced {
                    finished = true;
                }
                if !dropped {
                    i += 1;
                }
            } else if exists {
                if let Found::Chain(chain) =
                    find(&mut self.chains, packet.chain_id, packet.forked_id.as_ref())
                {
                    Self::sync(chain, &packet, now, host);
                }
                changed = true;
                self.queue.remove(i);
                self.queue_time = 0;
            } else if desynced {
                host.cancel(packet.chain_id, packet.forked_id.clone());
                self.queue.remove(i);
            } else {
                i += 1;
            }
        }
        if desynced && !self.queue.is_empty() {
            let mut keep = VecDeque::new();
            for v in std::mem::take(&mut self.queue) {
                let known = matches!(
                    find(&mut self.chains, v.chain_id, v.forked_id.as_ref()),
                    Found::Chain(_)
                );
                if !known && v.initial {
                    host.cancel(v.chain_id, v.forked_id);
                } else {
                    keep.push_back(v);
                }
            }
            self.queue = keep;
        }
        changed
    }

    /// Applies a client packet to a chain the server has.
    fn sync(chain: &mut Chain, packet: &p::SyncInteractionChain, now: i64, host: &mut dyn Host) {
        for f in packet.new_forks.iter().flatten() {
            let (Some(id), Some(_)) = (&f.forked_id, &f.data) else {
                continue;
            };
            let mut base = id;
            while let Some(n) = &base.forked_id {
                base = n;
            }
            let key = fork_key(base);
            if chain.forks.iter().any(|e| e.0 == key) {
                Self::sync(chain.fork_mut(key).unwrap(), f, now, host);
                continue;
            }
            let gone = match chain.entry_pos(base.entry_index) {
                Some(e) => base.sub_index < chain.entries[e].server.lock().unwrap().total_forks,
                None => base.entry_index < chain.index_offset,
            };
            if !gone {
                let t = chain.temp.entry(key).or_default();
                t.fork_id = f.forked_id.clone();
                t.data = f.data.clone();
                sync_temp(t, f);
            }
        }
        let Some(states) = &packet.interaction_data else {
            chain.client_state = packet.state;
            return;
        };
        if packet.operation_base_index < 0 {
            return;
        }
        for (i, d) in states.iter().enumerate() {
            let Some(d) = d else { continue };
            let index = packet.operation_base_index + i as i32;
            if index > chain.temp_sync_offset + chain.temp_sync.len() as i32 {
                continue;
            }
            let Some(e) = chain.entry_pos(index) else {
                chain.put_sync(index, d.clone());
                continue;
            };
            let entry = &mut chain.entries[e];
            let back = entry
                .client
                .as_ref()
                .is_some_and(|c| c.state != St::NOT_FINISHED)
                && d.state == St::NOT_FINISHED;
            if back || !entry.set_client(Some(d.clone())) {
                entry.desynced = true;
                chain.clear_sync(index);
                chain.flag_desync();
                return;
            }
            if chain.temp_sync_offset == index {
                chain.temp_sync_offset = index + 1;
            }
            Self::remove_if_finished(chain, e, now, host);
        }
        chain.clear_sync(packet.operation_base_index + states.len() as i32);
        chain.client_state = packet.state;
    }

    fn cancel_chains(chain: &mut Chain, host: &mut dyn Host) {
        chain.server_state = St::FAILED;
        chain.client_state = St::FAILED;
        host.cancel(chain.id, chain.fork_id.clone());
        for f in &mut chain.forks {
            Self::cancel_chains(&mut f.1, host);
        }
    }

    fn remove_if_finished(chain: &mut Chain, e: usize, now: i64, host: &mut dyn Host) {
        let (index, state) = (chain.entries[e].index, chain.entries[e].state());
        if chain.op_index == index && state != St::NOT_FINISHED {
            chain.final_state = state;
        }
        let client_ended = chain.entries[e]
            .client
            .as_ref()
            .is_some_and(|c| c.state != St::NOT_FINISHED);
        if state != St::NOT_FINISHED {
            if !chain.requires_client || client_ended {
                if e != 0 {
                    chain.flag_desync();
                } else {
                    chain.entries.remove(0);
                    chain.index_offset += 1;
                    let id = chain.id;
                    chain.temp.retain(|k, t| {
                        if k.0 != index {
                            return true;
                        }
                        host.cancel(id, t.fork_id.clone());
                        false
                    });
                }
            }
        } else if client_ended && !host.waiting_for_client() {
            let entry = &mut chain.entries[e];
            if entry.waiting_server == 0 {
                entry.waiting_server = now;
            }
            if (now - entry.waiting_server) / 1_000_000 > host.timeout_ms() {
                Self::cancel_chains(chain, host);
            }
        }
    }

    /// Whether a root is on cooldown for a chain the client starts.
    fn on_cooldown(&mut self, root: &Root, ty: IT, host: &dyn Host, remote: bool) -> bool {
        let mut id = root.id.clone();
        let mut time = match ty {
            IT::COLLISION_ENTER
            | IT::COLLISION_LEAVE
            | IT::PROJECTILE_SPAWN
            | IT::PROJECTILE_HIT
            | IT::PROJECTILE_MISS
            | IT::PROJECTILE_BOUNCE
            | IT::GAME_MODE_SWAP
            | IT::ENTITY_STAT_EFFECT
            | IT::PICKUP
            | IT::ON_BREAK
            | IT::ON_BREAK_IMPACT => 0.0,
            _ => 0.35,
        };
        let mut charges = vec![0.0f32];
        let mut interrupt = false;
        let mode = if host.creative() {
            p::GameMode::CREATIVE
        } else {
            p::GameMode::ADVENTURE
        };
        let settings = root.settings.iter().find(|s| s.0 == mode).map(|s| &s.1);
        let mut bypass = false;
        for c in [
            root.cooldown.as_ref(),
            settings.and_then(|s| s.cooldown.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            time = c.cooldown;
            if let Some(t) = c.charge_times.as_ref().filter(|t| !t.is_empty()) {
                charges = t.clone();
            }
            if let Some(i) = &c.cooldown_id {
                id = i.clone();
            }
            interrupt |= c.interrupt_recharge;
            if c.click_bypass && remote {
                bypass = true;
                break;
            }
        }
        bypass |= remote && settings.is_some_and(|s| s.allow_skip_chain_on_click);
        let force = bypass
            || root
                .cooldown
                .as_ref()
                .is_none_or(|c| !c.skip_cooldown_reset);
        if !bypass && time <= 0.0 {
            return false;
        }
        if !force && !self.cooldowns.contains_key(&id) {
            return false;
        }
        let c = self.cooldowns.entry(id).or_insert_with(|| Cooldown {
            max: time,
            count: charges.len(),
            charges,
            interrupt,
            ..Default::default()
        });
        if bypass {
            c.remaining = c.max;
            c.count = c.charges.len();
            return false;
        }
        c.check()
    }

    /// The rules between a new root and the running chains: the chains it
    /// interrupts (by path), or `None` when one blocks it.
    fn rules(
        book: &Book,
        ty: IT,
        slot: i32,
        root: &Root,
        chains: &[&Chain],
        path: &mut Vec<(i32, i32)>,
        cancel: &mut Vec<(i32, Vec<(i32, i32)>)>,
    ) -> bool {
        for chain in chains {
            if !(chain.fork_id.is_none() || chain.predicted)
                || (ty == IT::EQUIPPED && chain.ty == IT::EQUIPPED && chain.ctx.slot != slot)
            {
                continue;
            }
            if chain.server_state == St::NOT_FINISHED
                && let Some(current) = book.root(chain.root)
            {
                let op = match current.ops.get(chain.counter) {
                    Some(Op::Run { inter, .. }) => book.inter_at(*inter),
                    _ => None,
                };
                let op = op.and_then(|o| o.rules.as_ref().map(|r| (r, &o.tags)));
                if root
                    .rules
                    .interrupts(ty, &root.tags, chain.ty, &current.tags, &current.rules)
                    || op.is_some_and(|(r, tags)| {
                        root.rules.interrupts(ty, &root.tags, chain.ty, tags, r)
                    })
                {
                    cancel.push((chain.id, path.clone()));
                } else if root.rules.blocked(
                    ty,
                    &root.tags,
                    chain.ty,
                    &current.tags,
                    &current.rules,
                ) || op
                    .is_some_and(|(r, tags)| root.rules.blocked(ty, &root.tags, chain.ty, tags, r))
                {
                    return false;
                }
            }
            if cancel.is_empty() {
                let forks: Vec<&Chain> = chain.forks.iter().map(|f| &f.1).collect();
                for (n, f) in forks.iter().enumerate() {
                    path.push(chain.forks[n].0);
                    let ok = Self::rules(book, ty, slot, root, &[f], path, cancel);
                    path.pop();
                    if !ok {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Starts the chain a client's first packet names. False when it has
    /// to wait in the queue.
    fn start(
        &mut self,
        book: &Book,
        host: &mut dyn Host,
        packet: &p::SyncInteractionChain,
    ) -> bool {
        let index = packet.chain_id;
        let cancel = |host: &mut dyn Host| {
            tracing::debug!(
                "chain {index} refused at start ({:?}, held {:?}, slots {} {})",
                packet.interaction_type,
                packet.item_in_hand_id,
                packet.active_hotbar_slot,
                packet.active_utility_slot
            );
            host.cancel(index, packet.forked_id.clone());
            true
        };
        if !packet.initial {
            if packet.forked_id.is_none() {
                host.cancel(index, None);
            }
            return true;
        }
        let Some(data) = packet.data.clone().filter(|_| packet.forked_id.is_none()) else {
            return cancel(host);
        };
        if index <= 0 || index <= self.last_client {
            return cancel(host);
        }
        let ty = packet.interaction_type;
        let (section, slot) = match ty {
            IT::EQUIPPED => (-3, packet.equip_slot),
            IT::HELD_OFFHAND => (-5, host.active_slot(-5)),
            _ => (-1, host.active_slot(-1)),
        };
        let held = host.item_at(section, slot);
        let Some(root_index) = book.held_root(held.as_deref(), ty) else {
            tracing::debug!("chain {index}: no root for {held:?} in slot {section}/{slot}");
            return cancel(host);
        };
        let Some(root) = book.root(root_index) else {
            return false;
        };
        if root.ops.is_empty() {
            return cancel(host);
        }
        let mut interrupted = Vec::new();
        {
            let chains: Vec<&Chain> = self.chains.iter().collect();
            if !Self::rules(
                book,
                ty,
                slot,
                root,
                &chains,
                &mut Vec::new(),
                &mut interrupted,
            ) {
                return false;
            }
        }
        for (id, path) in interrupted {
            let Some(mut c) = self.chains.iter_mut().find(|c| c.id == id) else {
                continue;
            };
            for k in path {
                match c.forks.iter().position(|f| f.0 == k) {
                    Some(n) => c = &mut c.forks[n].1,
                    None => break,
                }
            }
            Self::cancel_chains(c, host);
        }
        let (hotbar, utility) = (host.active_slot(-1), host.active_slot(-5));
        if packet.active_hotbar_slot != hotbar {
            cancel(host);
            host.set_active_slot(-1, hotbar);
            return true;
        }
        if packet.active_utility_slot != utility {
            cancel(host);
            host.set_active_slot(-5, utility);
            return true;
        }
        if host.item_at(-1, hotbar) != packet.item_in_hand_id
            || host.item_at(-5, utility) != packet.utility_item_id
        {
            return cancel(host);
        }
        if self.on_cooldown(root, ty, host, true) {
            return false;
        }
        let mut ctx = Ctx {
            section,
            slot,
            original: held.clone(),
            held,
            ..Default::default()
        };
        if let Some(b) = &data.block_position {
            let raw = (b.x, b.y, b.z);
            if host.block(raw).is_none() {
                return cancel(host);
            }
            ctx.target_block = Some(host.base_of(raw));
            ctx.target_raw = Some(raw);
        }
        if data.entity_id >= 0 {
            ctx.target_entity = Some(data.entity_id);
        }
        if data.target_slot != NONE {
            ctx.target_slot = Some(data.target_slot);
        }
        ctx.hit_location = data.hit_location;
        ctx.hit_detail = data.hit_detail.clone();
        let mut chain = Chain::new(ty, ctx, data, root_index);
        chain.id = index;
        Self::sync(&mut chain, packet, self.now, host);
        self.last_client = index;
        if !self.tick_chain(book, host, &mut chain, None) {
            chain.pre_ticked = true;
            self.chains.push(chain);
        }
        true
    }

    fn make_sync(chain: &mut Chain, base: i32, states: Option<Vec<Option<Data>>>) -> Sync {
        let mut forks = Vec::new();
        for key in std::mem::take(&mut chain.new_forks) {
            if let Some(f) = chain.fork_mut(key) {
                forks.push(Self::make_sync(f, 0, None));
            }
        }
        let s = Sync {
            initial_root: (!chain.sent_initial).then_some(chain.initial_root),
            ty: chain.ty,
            equip_slot: chain.ctx.slot,
            id: chain.id,
            fork_id: chain.fork_id.clone(),
            data: chain.data.clone(),
            state: chain.server_state,
            forks,
            base,
            states,
        };
        chain.sent_initial = true;
        s
    }

    fn send_sync(&mut self, chain: &mut Chain, base: i32, states: Vec<Option<Data>>) {
        if !chain.sent_initial || states.iter().any(Option::is_some) || !chain.new_forks.is_empty()
        {
            let s = Self::make_sync(chain, base, Some(states));
            self.out.push(s);
        }
    }

    /// Ticks a chain; true when it is over and can be dropped.
    fn tick_chain(
        &mut self,
        book: &Book,
        host: &mut dyn Host,
        chain: &mut Chain,
        top_first: Option<bool>,
    ) -> bool {
        if chain.pre_ticked {
            chain.pre_ticked = false;
            return false;
        }
        let first_of_top = top_first.unwrap_or(chain.is_first_run);
        let mut forks = std::mem::take(&mut chain.forks);
        forks.retain_mut(|f| !self.tick_chain(book, host, &mut f.1, Some(first_of_top)));
        forks.append(&mut chain.forks);
        chain.forks = forks;
        if chain.server_state != St::NOT_FINISHED {
            if chain.requires_client && chain.client_state == St::NOT_FINISHED {
                if !host.waiting_for_client() {
                    if chain.waiting_client == 0 {
                        chain.waiting_client = self.now;
                    }
                    if (self.now - chain.waiting_client) / 1_000_000 > host.timeout_ms() {
                        Self::cancel_chains(chain, host);
                        return chain.forks.is_empty();
                    }
                }
                return false;
            }
            self.finish(book, host, chain);
            return chain.forks.is_empty();
        }
        let base = chain.op_index;
        let mut states = Vec::new();
        if let Err(Cancelled(state)) = self.run(book, host, chain, top_first, &mut states) {
            chain.server_state = state;
            chain.client_state = state;
            chain.update_server_state(book);
            if chain.requires_client {
                self.send_sync(chain, base, states);
                host.cancel(chain.id, chain.fork_id.clone());
            }
        }
        if chain.server_state != St::NOT_FINISHED {
            if !chain.requires_client || chain.client_state != St::NOT_FINISHED {
                self.finish(book, host, chain);
                return chain.forks.is_empty();
            }
        } else if chain.client_state != St::NOT_FINISHED && !host.waiting_for_client() {
            if chain.waiting_server == 0 {
                chain.waiting_server = self.now;
            }
            if (self.now - chain.waiting_server) / 1_000_000 > host.timeout_ms() {
                Self::cancel_chains(chain, host);
            }
        }
        false
    }

    /// A chain that is over: the operation it was on is told it failed,
    /// and a client chain's cooldown gets one more frame.
    fn finish(&mut self, book: &Book, host: &mut dyn Host, chain: &mut Chain) {
        let max = book.root(chain.root).map_or(0, |r| r.ops.len());
        if chain.counter < max
            && let Some(e) = chain.entry_pos(chain.op_index)
        {
            chain.entries[e].server.lock().unwrap().state = St::FAILED;
            if let Some(c) = &mut chain.entries[e].client {
                c.state = St::FAILED;
            }
            if let Some(Op::Run { inter, .. }) =
                book.root(chain.root).and_then(|r| r.ops.get(chain.counter))
            {
                Self::handle(book, host, chain, e, *inter, false);
            }
            chain.counter = max;
        }
        if !chain.completed {
            chain.completed = true;
            if let Some(root) = book.root(chain.initial_root) {
                let id = root
                    .cooldown
                    .as_ref()
                    .and_then(|c| c.cooldown_id.clone())
                    .unwrap_or_else(|| root.id.clone());
                if let Some(c) = self.cooldowns.get_mut(&id) {
                    c.tick(0.016666668);
                }
            }
        }
    }

    /// Tells the players around that a step started or ended.
    fn handle(book: &Book, host: &mut dyn Host, chain: &Chain, e: usize, inter: i32, first: bool) {
        let Some(x) = book.inter_at(inter) else {
            return;
        };
        let state = chain.entries[e].state();
        let mut play = |cancel: bool| {
            host.play(
                p::PlayInteractionFor {
                    entity_id: host.network_id(),
                    chain_id: chain.id,
                    forked_id: chain.fork_id.clone(),
                    operation_index: chain.op_index,
                    interaction_id: inter,
                    interacted_item_id: None,
                    interaction_type: chain.base_ty,
                    cancel,
                },
                x.view_distance,
                !chain.requires_client,
            )
        };
        if state != St::NOT_FINISHED {
            if first && state == St::FINISHED {
                play(false);
            }
            play(true);
        } else if first {
            play(false);
        }
    }

    /// Runs a chain's operations for one tick, collecting the states to
    /// send.
    fn run(
        &mut self,
        book: &Book,
        host: &mut dyn Host,
        chain: &mut Chain,
        top_first: Option<bool>,
        states: &mut Vec<Option<Data>>,
    ) -> Result<(), Cancelled> {
        let ops = |c: &Chain| book.root(c.root).map_or(0, |r| r.ops.len());
        let mut max = ops(chain);
        let mut current = chain.counter;
        let base = chain.op_index;
        let mut depth = chain.call_stack.len();
        chain.is_first_run = chain.first_run;
        chain.first_run = false;
        if chain.is_first_run {
            match top_first {
                None => chain.time_shift = self.shift[chain.ty.0 as usize % TYPES],
                Some(first) => chain.is_first_run = first,
            }
        } else {
            chain.time_shift = 0.0;
        }
        loop {
            if current >= max {
                while depth > 0 && current >= max {
                    chain.pop_root();
                    depth = chain.call_stack.len();
                    current = chain.counter;
                    max = ops(chain);
                }
                if depth == 0 && current >= max {
                    break;
                }
            }
            states.push(self.server_tick(book, host, chain)?);
            if depth != chain.call_stack.len() {
                depth = chain.call_stack.len();
                max = ops(chain);
            } else if current == chain.counter {
                break;
            }
            chain.op_index += 1;
            current = chain.counter;
            if current >= max {
                while depth > 0 {
                    chain.pop_root();
                    depth = chain.call_stack.len();
                    current = chain.counter;
                    max = ops(chain);
                    if current < max || depth == 0 {
                        break;
                    }
                }
                if depth == 0 && current >= max {
                    break;
                }
            }
        }
        chain.update_server_state(book);
        if chain.requires_client {
            self.send_sync(chain, base, std::mem::take(states));
        }
        Ok(())
    }

    /// One operation of a chain, for the entry at its operation index.
    fn server_tick(
        &mut self,
        book: &Book,
        host: &mut dyn Host,
        chain: &mut Chain,
    ) -> Result<Option<Data>, Cancelled> {
        let op = book
            .root(chain.root)
            .and_then(|r| r.ops.get(chain.counter))
            .cloned()
            .unwrap_or(Op::Jump(u32::MAX));
        let index = chain.op_index;
        let e = chain.entry_or_new(index);
        let mut ret = None;
        let mut wrong = std::mem::take(&mut chain.entries[e].desynced);
        if chain.entries[e].client.is_none() {
            let d = chain.take_sync(index);
            wrong |= !chain.entries[e].set_client(d);
        }
        if wrong {
            ret = Some(chain.entries[e].server.clone());
            chain.flag_desync();
            chain.clear_sync(index);
        }
        let wait = match &op {
            Op::Run { inter, .. } => book.inter_at(*inter).map_or(W::NONE, Self::waits),
            Op::Jump(_) => W::NONE,
        };
        if wait == W::CLIENT && chain.entries[e].client.is_none() {
            if host.waiting_for_client() {
                return Ok(None);
            }
            let entry = &mut chain.entries[e];
            if entry.waiting_sync == 0 {
                entry.waiting_sync = self.now;
            }
            if (self.now - entry.waiting_sync) / 1_000_000 > host.timeout_ms() {
                chain.server_state = St::FAILED;
                chain.client_state = St::FAILED;
                Self::cancel_chains(chain, host);
                return Ok(None);
            }
            if std::mem::take(&mut entry.send_initial) || wrong {
                ret = Some(entry.server.clone());
            }
            return Ok(ret);
        }
        let before = {
            let mut s = chain.entries[e].server.lock().unwrap().clone();
            s.progress = s.progress.trunc();
            s
        };
        let mut time = chain.entries[e].time(self.now);
        let mut first = false;
        if chain.entries[e].timestamp == 0 {
            time = chain.time_shift;
            chain.entries[e].timestamp = self.now - (time * 1.0e9) as i64;
            first = true;
        }
        match &op {
            Op::Jump(to) => {
                chain.counter = *to as usize;
                chain.entries[e].server.lock().unwrap().state = St::FINISHED;
            }
            Op::Run { inter, labels } => {
                self.tick_op(book, host, chain, e, *inter, labels, first, time)?;
            }
        }
        let after = {
            let mut s = chain.entries[e].server.lock().unwrap().clone();
            s.progress = s.progress.trunc();
            s
        };
        if first || before != after {
            ret = Some(chain.entries[e].server.clone());
        }
        if let Op::Run { inter, .. } = &op {
            Self::handle(book, host, chain, e, *inter, first);
        }
        Self::remove_if_finished(chain, e, self.now, host);
        Ok(ret)
    }

    /// Where an operation waits for data. A type the server does not run
    /// follows the client, so it waits for it unless the client is the one
    /// waiting.
    fn waits(x: &Inter) -> W {
        match (&x.kind, x.wait) {
            (Kind::Other { .. }, W::SERVER) => W::SERVER,
            (Kind::Other { .. }, _) => W::CLIENT,
            (_, w) => w,
        }
    }
}

impl Manager {
    fn set_shift(&mut self, chain: &mut Chain, shift: f32) {
        chain.time_shift = shift;
        if chain.fork_id.is_none() {
            let t = chain.ty.0 as usize % TYPES;
            self.shift[t] = shift;
            self.shift_dirty[t] = true;
            self.shifts_dirty = true;
        }
    }

    /// Enters another root: the chain continues there and comes back after
    /// this operation.
    fn execute(chain: &mut Chain, e: usize, root: i32) {
        chain.entries[e]
            .server
            .lock()
            .unwrap()
            .entered_root_interaction = root;
        chain.call_stack.push((chain.root, chain.counter));
        chain.counter = 0;
        chain.root = root;
    }

    /// A step forks a chain of its own.
    #[allow(clippy::too_many_arguments)]
    fn fork(
        chain: &mut Chain,
        e: usize,
        ty: IT,
        mut ctx: Ctx,
        root: i32,
        predicted: bool,
        data: Option<p::InteractionChainData>,
    ) -> (i32, i32) {
        let mut data = data.unwrap_or_else(|| chain.data.clone());
        // A chain without a target slot gets slot 0 the first time one
        // is asked for.
        let slot = match ctx.target_slot {
            Some(s) => s,
            None => *chain.ctx.target_slot.get_or_insert(0),
        };
        ctx.target_slot = Some(slot);
        data.target_slot = slot;
        if let Some(t) = ctx.target_entity {
            data.entity_id = t;
        }
        if let Some(h) = ctx.hit_location {
            data.hit_location = Some(h);
        }
        if let Some(h) = &ctx.hit_detail {
            data.hit_detail = Some(h.clone());
        }
        if let Some((x, y, z)) = ctx.target_raw {
            data.block_position = Some(p::BlockPosition { x, y, z });
        }
        let (fork_id, key) = chain.next_fork_id(e);
        let mut f = Chain::new(ty, ctx, data, root);
        f.fork_id = Some(fork_id);
        f.base_fork = Some(key);
        f.id = chain.id;
        f.base_ty = chain.base_ty;
        f.predicted = predicted;
        f.skip_on_click = chain.skip_on_click;
        f.time_shift = chain.time_shift;
        if let Some(t) = chain.temp.remove(&key) {
            f.client_state = t.client_state.unwrap_or(St::NOT_FINISHED);
            f.temp_sync.extend(t.sync);
            f.temp.extend(t.forks);
        }
        chain.new_forks.push(key);
        chain.forks.push((key, f));
        key
    }

    /// One tick of an interaction, with what every type shares around its
    /// own logic.
    #[allow(clippy::too_many_arguments)]
    fn tick_op(
        &mut self,
        book: &Book,
        host: &mut dyn Host,
        chain: &mut Chain,
        e: usize,
        inter: i32,
        labels: &[u32],
        first: bool,
        time: f32,
    ) -> Result<(), Cancelled> {
        let Some(x) = book.inter_at(inter) else {
            chain.entries[e].server.lock().unwrap().state = St::FINISHED;
            chain.counter += 1;
            return Ok(());
        };
        let mode = if host.creative() {
            p::GameMode::CREATIVE
        } else {
            p::GameMode::ADVENTURE
        };
        chain.skip_on_click |= book.root(chain.root).is_some_and(|r| {
            r.settings
                .iter()
                .any(|s| s.0 == mode && s.1.allow_skip_chain_on_click)
        });
        let (counter, depth) = (chain.counter, chain.call_stack.len());
        // The held item must still be the one the chain started with.
        let active = if chain.ty != IT::EQUIPPED && chain.ctx.section != -11 {
            host.active_slot(chain.ctx.section)
        } else {
            chain.ctx.slot
        };
        let same_item = x.item_change == p::InteractionItemChangeBehavior::IGNORE
            || ((chain.ty == IT::EQUIPPED || active == chain.ctx.slot)
                && (chain.ctx.slot == -1
                    || (chain.ctx.held.is_some()
                        && host.item_at(chain.ctx.section, chain.ctx.slot) == chain.ctx.held)
                    || (chain.ctx.held.is_none()
                        && host.item_at(chain.ctx.section, chain.ctx.slot).is_none())));
        let mut changed = false;
        let mut shift = None;
        {
            let mut s = chain.entries[e].server.lock().unwrap();
            if same_item {
                if !failed(s.state) {
                    if time < x.run_time {
                        s.state = St::NOT_FINISHED;
                    } else {
                        if x.run_time > 0.0 {
                            shift = Some(time - x.run_time);
                        }
                        s.state = St::FINISHED;
                    }
                    s.progress = time;
                }
            } else {
                s.state = match x.item_change {
                    p::InteractionItemChangeBehavior::FAIL => St::FAILED,
                    p::InteractionItemChangeBehavior::FINISH => St::FINISHED,
                    _ => St::ITEM_CHANGED,
                };
                s.progress = time;
                changed = s.state == St::ITEM_CHANGED;
            }
        }
        if shift.is_some() {
            chain.entries[e].shift = shift;
        }
        if !changed {
            self.tick0(book, host, chain, e, x, labels, first, time);
        }
        let state = {
            let mut s = chain.entries[e].server.lock().unwrap();
            if !chain.is_first_run
                && s.state == St::NOT_FINISHED
                && (chain.skip_on_click || x.skip_on_click.contains(&mode))
                && chain.entries[e]
                    .client
                    .as_ref()
                    .is_some_and(|c| c.state == St::SKIP)
            {
                s.state = St::SKIP;
                s.progress = time;
            }
            s.state
        };
        match state {
            St::FAILED | St::FINISHED | St::SKIP => {
                if let Some(shift) = chain.entries[e].shift {
                    self.set_shift(chain, shift);
                }
                if chain.counter == counter && chain.call_stack.len() == depth {
                    chain.counter += 1;
                }
                Ok(())
            }
            St::ITEM_CHANGED => Err(Cancelled(state)),
            _ => Ok(()),
        }
    }

    /// What an interaction type does in a tick.
    #[allow(clippy::too_many_arguments)]
    fn tick0(
        &mut self,
        book: &Book,
        host: &mut dyn Host,
        chain: &mut Chain,
        e: usize,
        x: &Inter,
        labels: &[u32],
        first: bool,
        time: f32,
    ) {
        let set = |chain: &Chain, s: St| chain.entries[e].server.lock().unwrap().state = s;
        let state = |chain: &Chain| chain.entries[e].state();
        // The end every plain type shares: a failed step goes to its
        // failed branch.
        let tail = |chain: &mut Chain| {
            if chain.entries[e].state() == St::FAILED
                && let Some(l) = labels.first()
            {
                chain.counter = *l as usize;
            }
        };
        let client = chain.entries[e].client.clone();
        match &x.kind {
            Kind::Simple | Kind::Serial(_) => tail(chain),
            Kind::Condition { mode, states } => {
                let m = host.movement();
                let now = [
                    m.jumping,
                    m.swimming,
                    m.crouching,
                    m.running,
                    m.flying,
                    m.gliding,
                ];
                let mode_ok = mode.is_none_or(|g| (g == p::GameMode::CREATIVE) == host.creative());
                let ok = mode_ok
                    && states
                        .iter()
                        .zip(now)
                        .all(|(want, is)| want.is_none_or(|w| w == is));
                set(chain, if ok { St::FINISHED } else { St::FAILED });
                tail(chain);
            }
            Kind::Parallel(roots) => {
                if let Some(&r) = roots.first() {
                    Self::execute(chain, e, r);
                }
                for &r in roots.iter().skip(1) {
                    Self::fork(chain, e, chain.ty, chain.ctx.clone(), r, true, None);
                }
                set(chain, St::FINISHED);
            }
            Kind::Repeat { fork, repeat } => {
                if first && *repeat != -1 {
                    chain.entries[e].repeats = *repeat;
                }
                if let Some(key) = chain.entries[e].fork {
                    let s = chain
                        .forks
                        .iter()
                        .find(|f| f.0 == key)
                        .map_or(St::FINISHED, |f| f.1.server_state);
                    match s {
                        St::NOT_FINISHED => return set(chain, St::NOT_FINISHED),
                        St::FINISHED => {
                            if *repeat != -1 && chain.entries[e].repeats <= 0 {
                                set(chain, St::FINISHED);
                                return tail(chain);
                            }
                            set(chain, St::NOT_FINISHED);
                        }
                        St::FAILED => {
                            set(chain, St::FAILED);
                            return tail(chain);
                        }
                        _ => {}
                    }
                }
                let key = Self::fork(chain, e, chain.ty, chain.ctx.clone(), *fork, true, None);
                chain.entries[e].fork = Some(key);
                set(chain, St::NOT_FINISHED);
                if *repeat != -1 {
                    chain.entries[e].repeats -= 1;
                }
            }
            Kind::Replace { var, default } => {
                if failed(state(chain)) || !first {
                    return;
                }
                let next = chain
                    .ctx
                    .original
                    .as_ref()
                    .and_then(|k| book.items.get(k))
                    .and_then(|i| i.vars.get(var))
                    .copied()
                    .or((*default != NONE).then_some(*default));
                match next {
                    Some(r) => {
                        set(chain, St::FINISHED);
                        Self::execute(chain, e, r);
                    }
                    None => set(chain, St::FAILED),
                }
            }
            Kind::UseBlock | Kind::PickBlock | Kind::BreakBlock { .. } | Kind::Door { .. } => {
                if first {
                    self.block_step(book, host, chain, e, x, &client);
                    tail(chain);
                }
                if matches!(x.kind, Kind::BreakBlock { .. })
                    && let Some(t) = chain.ctx.target_block
                    && let Some((id, rot)) = host.cell(t)
                {
                    let mut s = chain.entries[e].server.lock().unwrap();
                    s.block_position = Some(p::BlockPosition {
                        x: t.0,
                        y: t.1,
                        z: t.2,
                    });
                    s.placed_block_id = id;
                    s.block_rotation = Some(rot);
                }
            }
            Kind::Charging { next, forks, .. } => {
                let Some(c) = client else { return };
                if c.state == St::FAILED && !labels.is_empty() {
                    set(chain, St::FAILED);
                    if let Some(l) = labels.get(next.len()) {
                        chain.counter = *l as usize;
                    }
                    return;
                }
                if let Some(wanted) = c.fork_counts.as_ref().filter(|_| !forks.is_empty()) {
                    let mut running = chain.entries[e].fork.filter(|k| {
                        chain
                            .forks
                            .iter()
                            .any(|f| f.0 == *k && f.1.server_state == St::NOT_FINISHED)
                    });
                    let mut matches = true;
                    for &(ty, want) in wanted {
                        let mut made = chain.entries[e]
                            .counts
                            .iter()
                            .find(|x| x.0 == ty)
                            .map_or(0, |x| x.1);
                        if running.is_none()
                            && made < want
                            && let Some(root) = forks.iter().find(|f| f.0 == ty).map(|f| f.1)
                        {
                            let key = Self::fork(chain, e, ty, chain.ctx.clone(), root, true, None);
                            running = Some(key);
                            chain.entries[e].fork = Some(key);
                            made += 1;
                            chain.entries[e].counts.retain(|x| x.0 != ty);
                            chain.entries[e].counts.push((ty, made));
                        }
                        matches &= made == want;
                    }
                    if !matches {
                        return set(chain, St::NOT_FINISHED);
                    }
                }
                if c.charge_value == -1.0 {
                    set(chain, St::NOT_FINISHED);
                } else {
                    set(chain, St::FINISHED);
                    if c.charge_value != -2.0 {
                        // The longest charge time reached.
                        let reached = next.iter().rposition(|k| c.charge_value >= k.0);
                        if let Some(l) = reached.and_then(|n| labels.get(n)) {
                            chain.counter = *l as usize;
                        }
                    }
                }
            }
            Kind::ModifyInventory(m) => {
                if first {
                    if !host.modify_inventory(m, &mut chain.ctx) {
                        set(chain, St::FAILED);
                    }
                    tail(chain);
                }
            }
            Kind::ChangeStat {
                stats,
                percent,
                behaviour,
                user,
                ability_cost,
            } => {
                if first {
                    if *user && !*ability_cost {
                        host.change_stats(stats, *percent, *behaviour);
                    }
                    tail(chain);
                }
            }
            Kind::StatsCondition {
                costs,
                less_than,
                lenient,
                percent,
                ability_cost,
            } => {
                if first {
                    let afford = *ability_cost
                        || costs.iter().all(|&(i, cost)| {
                            let Some((value, min, max)) = host.stat(i) else {
                                return false;
                            };
                            let v = if *percent {
                                if max > min {
                                    (value - min) / (max - min) * 100.0
                                } else {
                                    0.0
                                }
                            } else {
                                value
                            };
                            if *less_than {
                                v < cost
                            } else {
                                v >= cost || (*lenient && v > 0.0 && min < 0.0)
                            }
                        });
                    if !afford {
                        set(chain, St::FAILED);
                    }
                    tail(chain);
                }
            }
            Kind::ApplyEffect {
                effect,
                duration,
                user,
            } => {
                if first {
                    if *user {
                        host.apply_effect(*effect, (*duration > 0.0).then_some(*duration));
                    }
                    tail(chain);
                }
            }
            Kind::EffectCondition { effects, all, user } => {
                if first {
                    if *user && effects.iter().any(|&i| host.has_effect(i) != *all) {
                        set(chain, St::FAILED);
                    }
                    tail(chain);
                }
            }
            Kind::ClearEffect { effect, user } => {
                if first {
                    if *user {
                        host.clear_effect(*effect);
                    }
                    tail(chain);
                }
            }
            Kind::ChangeActiveSlot(target) => {
                if !first {
                    return set(chain, St::FINISHED);
                }
                let slot = if *target == NONE {
                    chain.ctx.target_slot.unwrap_or(0)
                } else {
                    if host.active_slot(-1) == *target {
                        return set(chain, St::FINISHED);
                    }
                    chain.ctx.target_slot = Some(*target);
                    *target
                };
                host.change_hotbar_slot(slot);
                let moved = chain.ctx.moved.take();
                if let Some(m) = &moved {
                    host.move_item(m);
                }
                // The item now held runs its swap-to interactions.
                let held = host.item_at(-1, slot);
                if let Some(root) = book.held_root(held.as_deref(), IT::SWAP_TO) {
                    let ctx = Ctx {
                        section: -1,
                        slot,
                        original: held.clone(),
                        held,
                        target_slot: (*target != NONE).then_some(slot),
                        ..Default::default()
                    };
                    Self::fork(chain, e, IT::SWAP_TO, ctx, root, moved.is_none(), None);
                }
                set(chain, St::FINISHED);
            }
            Kind::UseEntity => {
                if first {
                    // No entity gives a root of its own yet.
                    set(chain, St::FAILED);
                    tail(chain);
                }
            }
            Kind::PlaceModeSelect(branches) => {
                let mut i = host.place_mode();
                if i >= 6 || branches[i] == NONE {
                    i = 0;
                }
                if let Some(l) = labels.get(i) {
                    chain.counter = *l as usize;
                }
                let mut s = chain.entries[e].server.lock().unwrap();
                s.next_label = i as i32;
                s.state = St::FINISHED;
            }
            Kind::DragPlace {
                fork,
                per_tick,
                per_gesture,
            } => {
                let Some(c) = client else { return };
                if first {
                    chain.entries[e].forked = 0;
                }
                let mut made = chain.entries[e].forked;
                let wanted = c
                    .fork_counts
                    .iter()
                    .flatten()
                    .find(|f| f.0 == chain.ty)
                    .map_or(0, |f| f.1);
                if wanted > *per_gesture || wanted < made {
                    set(chain, St::FAILED);
                    return tail(chain);
                }
                for _ in 0..(wanted - made).min(*per_tick) {
                    Self::fork(chain, e, chain.ty, chain.ctx.clone(), *fork, true, None);
                    made += 1;
                }
                chain.entries[e].forked = made;
                if made >= wanted && c.state != St::NOT_FINISHED {
                    set(chain, c.state);
                    tail(chain);
                } else {
                    set(chain, St::NOT_FINISHED);
                }
            }
            Kind::PlaceBlock {
                block,
                remove_item,
                quick_replace,
                quick_retype,
                no_physics,
            } => {
                let Some(c) = client else { return };
                if !first {
                    return set(chain, c.state);
                }
                if let (Some(pos), Some(rot)) = (&c.block_position, &c.block_rotation) {
                    let at = (pos.x, pos.y, pos.z);
                    if host.block(at).is_none() || !host.in_reach(chain.ctx.held.as_deref(), at) {
                        return set(chain, St::FAILED);
                    }
                    let r = PlaceRequest {
                        pos: at,
                        rotation: rot.clone(),
                        face: c.block_face,
                        client_block: c.placed_block_id,
                        block: *block,
                        remove_item: *remove_item,
                        quick_replace: *quick_replace,
                        quick_retype: *quick_retype,
                        no_physics: *no_physics,
                        slot: chain.ctx.slot,
                    };
                    match host.place(&r) {
                        None => return set(chain, St::FAILED),
                        Some(used_up) => {
                            if used_up {
                                chain.ctx.held = None;
                            }
                        }
                    }
                    if let Some((id, rot)) = host.cell(at) {
                        let mut s = chain.entries[e].server.lock().unwrap();
                        s.block_position = Some(pos.clone());
                        s.placed_block_id = id;
                        s.block_rotation = Some(rot);
                    }
                }
                tail(chain);
            }
            Kind::Chaining { next, flags, .. } => {
                let Some(c) = client else { return };
                if c.flag_index != -1 {
                    if c.flag_index >= 0 && (c.flag_index as usize) < flags.len() {
                        set(chain, St::FINISHED);
                        if let Some(l) = labels.get(next.len() + c.flag_index as usize) {
                            chain.counter = *l as usize;
                        }
                    } else {
                        set(chain, St::FAILED);
                    }
                } else if c.chaining_index == -1 {
                    set(chain, St::NOT_FINISHED);
                } else if c.chaining_index >= 0 && (c.chaining_index as usize) < next.len() {
                    set(chain, St::FINISHED);
                    if let Some(l) = labels.get(c.chaining_index as usize) {
                        chain.counter = *l as usize;
                    }
                } else {
                    set(chain, St::FAILED);
                }
            }
            Kind::Select {
                fail_on,
                hit_entity,
                hit_block,
            } => {
                // The entities and blocks a selector hits fork chains of
                // their own (combat): not made yet, nothing is hit.
                let labelled = !labels.is_empty();
                let fails = |on: p::FailOnType| *fail_on == on || *fail_on == p::FailOnType::EITHER;
                if *hit_entity
                    && labelled
                    && state(chain) == St::FINISHED
                    && fails(p::FailOnType::ENTITY)
                {
                    set(chain, St::FAILED);
                }
                if *hit_block
                    && labelled
                    && state(chain) == St::FINISHED
                    && fails(p::FailOnType::BLOCK)
                {
                    set(chain, St::FAILED);
                }
                if state(chain) != St::FAILED
                    && let Some(c) = &client
                {
                    set(chain, c.state);
                }
                tail(chain);
            }
            Kind::Other { .. } => self.follow(book, host, chain, e, x, labels, first, time, client),
        }
    }

    /// The part block types share: the target must be a block in reach.
    fn block_step(
        &mut self,
        book: &Book,
        host: &mut dyn Host,
        chain: &mut Chain,
        e: usize,
        x: &Inter,
        client: &Option<p::InteractionSyncData>,
    ) {
        let fail = |chain: &Chain| chain.entries[e].server.lock().unwrap().state = St::FAILED;
        if x.use_latest {
            let Some(b) = client.as_ref().and_then(|c| c.block_position.as_ref()) else {
                return fail(chain);
            };
            let raw = (b.x, b.y, b.z);
            chain.ctx.target_block = Some(host.base_of(raw));
            chain.ctx.target_raw = Some(raw);
        }
        let Some(target) = chain.ctx.target_block else {
            return fail(chain);
        };
        if !host.in_reach(chain.ctx.held.as_deref(), target) {
            tracing::debug!("chain {}: {target:?} out of reach", chain.id);
            return fail(chain);
        }
        let Some(block) = host.block(target).filter(|&b| b != 0 && b != 1) else {
            tracing::debug!("chain {}: no block at {target:?}", chain.id);
            return fail(chain);
        };
        match &x.kind {
            Kind::UseBlock => match book.block_root(block, chain.ty) {
                Some(root) if book.root(root).is_some() => {
                    chain.entries[e].server.lock().unwrap().state = St::FINISHED;
                    Self::execute(chain, e, root);
                }
                _ => fail(chain),
            },
            Kind::Door { horizontal } => {
                if !host.door(target, *horizontal) {
                    fail(chain);
                }
            }
            Kind::BreakBlock {
                harvest,
                soft_only,
                tool,
                match_tool,
            } => {
                if !host.break_block(
                    target,
                    &chain.ctx,
                    *harvest,
                    *soft_only,
                    tool.as_deref(),
                    *match_tool,
                ) {
                    fail(chain);
                }
            }
            _ => {}
        }
    }

    /// A type the server does not run yet: the step takes the client's
    /// state and the chain goes where the client's went.
    #[allow(clippy::too_many_arguments)]
    fn follow(
        &mut self,
        book: &Book,
        host: &mut dyn Host,
        chain: &mut Chain,
        e: usize,
        x: &Inter,
        labels: &[u32],
        first: bool,
        time: f32,
        client: Option<p::InteractionSyncData>,
    ) {
        if let Kind::Other { block: true, .. } = &x.kind {
            // A block type: the target must be a block in reach; then the
            // step ends by its run time, taking the client's verdict when
            // the client already has one.
            if !first {
                return;
            }
            self.block_step(book, host, chain, e, x, &client);
            if chain.entries[e].state() != St::FAILED {
                host.other(&x.ty, &chain.ctx);
                if let Some(c) = client.filter(|c| c.state != St::NOT_FINISHED) {
                    chain.entries[e].server.lock().unwrap().state = c.state;
                }
            }
            if chain.entries[e].state() == St::FAILED
                && let Some(l) = labels.first()
            {
                chain.counter = *l as usize;
            }
            return;
        }
        if first {
            host.other(&x.ty, &chain.ctx);
        }
        // Where the client waits for the server there is nothing to
        // follow.
        let Some(c) = client.filter(|_| x.wait != W::SERVER) else {
            // The client waits for the server here: the step ends by its
            // run time alone.
            if chain.entries[e].state() == St::FAILED
                && let Some(l) = labels.first()
            {
                chain.counter = *l as usize;
            }
            return;
        };
        // Forks the client made for this step.
        let index = chain.entries[e].index;
        let mut keys: Vec<(i32, i32)> = chain
            .temp
            .iter()
            .filter(|(k, t)| k.0 == index && t.root != NONE && book.root(t.root).is_some())
            .map(|(k, _)| *k)
            .collect();
        keys.sort();
        for k in keys {
            let made = chain.entries[e].server.lock().unwrap().total_forks;
            if k.1 != made {
                break;
            }
            let (root, data) = {
                let t = &chain.temp[&k];
                (t.root, t.data.clone())
            };
            Self::fork(chain, e, chain.ty, chain.ctx.clone(), root, true, data);
        }
        {
            let mut s = chain.entries[e].server.lock().unwrap();
            s.state = c.state;
            s.progress = time;
        }
        if c.state == St::NOT_FINISHED {
            return;
        }
        if c.entered_root_interaction != NONE && book.root(c.entered_root_interaction).is_some() {
            return Self::execute(chain, e, c.entered_root_interaction);
        }
        // Where the client went next: its state for the following step
        // names the operation.
        let next = chain
            .temp_sync
            .get((index + 1 - chain.temp_sync_offset).max(0) as usize)
            .and_then(|d| d.as_ref())
            .filter(|_| index + 1 >= chain.temp_sync_offset);
        match next {
            Some(n) if n.root_interaction == chain.root => {
                chain.counter = n.operation_counter.max(0) as usize
            }
            Some(_) => {}
            None if chain.client_state == St::NOT_FINISHED && !labels.is_empty() => {
                // Not known yet: wait for the client's next step.
                chain.entries[e].server.lock().unwrap().state = St::NOT_FINISHED;
            }
            None => {
                if c.state == St::FAILED
                    && let Some(l) = labels.first()
                {
                    chain.counter = *l as usize;
                }
            }
        }
    }
}
