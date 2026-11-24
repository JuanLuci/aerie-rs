//! Game mode types (spec 30): what the server does when a player's game
//! mode changes, from the types in the assets.
//!
//! A type says what a mode does to a player beyond the mode itself: an
//! entity effect, markers, movement, interactions to run on entering. Aerie
//! has the two plain modes. Changing between them (`/gamemode`) sends the
//! client new movement settings (fly allowed or not) and runs the
//! new mode's "interactions on enter" as a server chain of type
//! `GameModeSwap`: the creative mode's chain applies a short entity effect
//! and the adventure mode's plays a sound.
//!
//! The rest of a type (markers, movement config, no-clip, overrides and so
//! on) is read and kept: [`Type::apply`] and [`Type::unapply`] list what
//! entering and leaving a type does, in the official order, for the day
//! something enters a type of its own (a spectator command).

use std::collections::HashMap;

use aerie_protocol::proto;
use serde_json::Value;

/// The name of the game mode table among the registry extras.
pub const TABLE: &str = "game-modes";
/// The movement settings of an adventure player (the extras name; the
/// creative ones are the join's).
pub const MOVEMENT_ADVENTURE: &str = "movement_settings_adventure";

/// A marker a type puts on the player, or takes away.
#[allow(dead_code)] // no caller yet: nothing enters a type of its own
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    Invulnerable,
    Intangible,
    PreventInventoryAccess,
    PreventEmotes,
}

/// One thing entering or leaving a type does to a player.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// The movement configuration of the type (entering), or the player's
    /// own again (leaving).
    Movement(Option<String>),
    Flying(bool),
    NoClip(bool),
    Marker(Marker, bool),
    TriggerBlocks(bool),
    /// Interaction overrides installed (entering) or removed (leaving), by
    /// interaction type name and root name.
    Overrides(Vec<(String, String)>, bool),
    /// The type's entity effect, by index: applied unless it is active
    /// already, or removed.
    Effect(i32, bool),
    /// A root interaction to run as a game mode swap chain.
    Run(String),
}

/// What the assets say of one type. Fields left out are `None`: the player
/// is left as it is.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct Type {
    pub on_enter: Option<String>,
    pub effect: Option<i32>,
    pub movement: Option<String>,
    pub flying: Option<bool>,
    pub no_clip: Option<bool>,
    pub invulnerable: Option<bool>,
    pub intangible: Option<bool>,
    pub prevent_inventory: Option<bool>,
    pub prevent_emotes: Option<bool>,
    pub trigger_blocks: Option<bool>,
    pub overrides: Vec<(String, String)>,
    pub override_all: bool,
}

#[allow(dead_code)]
impl Type {
    fn parse(v: &Value) -> Self {
        let b = |k: &str| v.get(k).and_then(Value::as_bool);
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        Self {
            on_enter: s("OnEnter"),
            effect: v
                .get("Effect")
                .and_then(Value::as_i64)
                .and_then(|i| i32::try_from(i).ok()),
            movement: s("MovementConfigId"),
            flying: b("Flying"),
            no_clip: b("NoClip"),
            invulnerable: b("Invulnerable"),
            intangible: b("Intangible"),
            prevent_inventory: b("PreventInventoryAccess"),
            prevent_emotes: b("PreventEmotes"),
            trigger_blocks: b("TriggerBlocks"),
            overrides: v
                .get("Overrides")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, r)| Some((k.clone(), r.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
            override_all: b("OverrideAllInteractions").unwrap_or(false),
        }
    }

    /// Entering the type: movement (then flying and no-clip), the markers,
    /// the interaction overrides, trigger blocks, the entity effect, and
    /// last the interactions on enter.
    pub fn apply(&self) -> Vec<Step> {
        let mut v = Vec::new();
        if let Some(m) = &self.movement {
            v.push(Step::Movement(Some(m.clone())));
        }
        if let Some(f) = self.flying {
            v.push(Step::Flying(f));
        }
        if let Some(n) = self.no_clip {
            v.push(Step::NoClip(n));
        }
        for (m, on) in self.markers() {
            v.push(Step::Marker(m, on));
        }
        if !self.overrides.is_empty() || self.override_all {
            v.push(Step::Overrides(self.overrides.clone(), true));
        }
        if let Some(t) = self.trigger_blocks {
            v.push(Step::TriggerBlocks(t));
        }
        if let Some(e) = self.effect {
            v.push(Step::Effect(e, true));
        }
        if let Some(r) = &self.on_enter {
            v.push(Step::Run(r.clone()));
        }
        v
    }

    /// Leaving the type: what it set goes back (flying off unless the mode
    /// is creative, no-clip off, `Invulnerable` back to the mode's own,
    /// the other markers off), the overrides come out, trigger blocks are
    /// on again and the entity effect is removed. The mode the player
    /// returns to then runs its own interactions on enter.
    pub fn unapply(&self, creative: bool) -> Vec<Step> {
        let mut v = Vec::new();
        if self.movement.is_some() {
            v.push(Step::Movement(None));
        }
        if self.flying.is_some() && !creative {
            v.push(Step::Flying(false));
        }
        if self.no_clip.is_some() {
            v.push(Step::NoClip(false));
        }
        if self.invulnerable.is_some() {
            v.push(Step::Marker(Marker::Invulnerable, creative));
        }
        if self.intangible.is_some() {
            v.push(Step::Marker(Marker::Intangible, false));
        }
        if self.prevent_inventory.is_some() {
            v.push(Step::Marker(Marker::PreventInventoryAccess, false));
        }
        if self.prevent_emotes.is_some() {
            v.push(Step::Marker(Marker::PreventEmotes, false));
        }
        if !self.overrides.is_empty() || self.override_all {
            v.push(Step::Overrides(self.overrides.clone(), false));
        }
        if self.trigger_blocks.is_some() {
            v.push(Step::TriggerBlocks(true));
        }
        if let Some(e) = self.effect {
            v.push(Step::Effect(e, false));
        }
        v
    }

    fn markers(&self) -> impl Iterator<Item = (Marker, bool)> {
        [
            (Marker::Invulnerable, self.invulnerable),
            (Marker::Intangible, self.intangible),
            (Marker::PreventInventoryAccess, self.prevent_inventory),
            (Marker::PreventEmotes, self.prevent_emotes),
        ]
        .into_iter()
        .filter_map(|(m, on)| Some((m, on?)))
    }
}

/// What a change of game mode asks of the world.
#[derive(Debug, Default, PartialEq)]
pub struct Change {
    /// The `UpdateMovementSettings` payload for the new mode, when known.
    pub movement: Option<Vec<u8>>,
    /// The name of the root interaction to run as a `GameModeSwap` chain.
    pub run: Option<String>,
}

/// The game mode types, and the movement settings of each plain mode.
#[derive(Default)]
pub struct Types {
    types: HashMap<String, Type>,
    creative_movement: Option<Vec<u8>>,
    adventure_movement: Option<Vec<u8>>,
}

impl Types {
    /// From the server table (one line per type, `key<TAB>{…}`).
    pub fn parse(table: &str) -> Self {
        let types = table
            .lines()
            .filter_map(|l| {
                let (k, j) = l.split_once('\t')?;
                Some((k.to_owned(), Type::parse(&serde_json::from_str(j).ok()?)))
            })
            .collect();
        Self {
            types,
            ..Default::default()
        }
    }

    /// The `UpdateMovementSettings` payloads of a creative and an
    /// adventure player.
    pub fn with_movement(mut self, creative: Option<Vec<u8>>, adventure: Option<Vec<u8>>) -> Self {
        self.creative_movement = creative;
        self.adventure_movement = adventure;
        self
    }

    pub fn get(&self, name: &str) -> Option<&Type> {
        self.types.get(name)
    }

    /// A player's game mode became `creative` (or adventure): the movement
    /// settings with fly allowed or not, and the mode's interactions on
    /// enter.
    pub fn on_change(&self, creative: bool) -> Change {
        let name = if creative { "Creative" } else { "Adventure" };
        Change {
            movement: if creative {
                &self.creative_movement
            } else {
                &self.adventure_movement
            }
            .clone(),
            run: self.get(name).and_then(|t| t.on_enter.clone()),
        }
    }
}

/// The `UpdateMovementSettings` packet for a payload.
pub fn movement_packet(payload: Vec<u8>) -> (u32, Vec<u8>) {
    (crate::join::settings_id::UPDATE_MOVEMENT_SETTINGS, payload)
}

/// The chain data of a game mode swap: no target.
pub fn chain_data() -> proto::InteractionChainData {
    proto::InteractionChainData {
        entity_id: -1,
        target_slot: i32::MIN,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE_TEXT: &str = "Adventure\t{\"OnEnter\":\"*Adventure_InteractionsOnEnter\"}\n\
Creative\t{\"OnEnter\":\"*Creative_InteractionsOnEnter\"}\n\
Ghost\t{\"Effect\":7,\"Flying\":true,\"NoClip\":true,\"Invulnerable\":true,\"Intangible\":true,\"PreventEmotes\":true,\"TriggerBlocks\":false,\"MovementConfigId\":\"Ghost\",\"OnEnter\":\"Go\"}\n";

    #[test]
    fn plain_modes_run_their_interactions_on_enter() {
        let t = Types::parse(TABLE_TEXT);
        assert_eq!(
            t.on_change(true).run.as_deref(),
            Some("*Creative_InteractionsOnEnter")
        );
        assert_eq!(
            t.on_change(false).run.as_deref(),
            Some("*Adventure_InteractionsOnEnter")
        );
        assert_eq!(t.get("Creative").unwrap().apply().len(), 1);
    }

    #[test]
    fn applying_goes_in_order_and_unapplying_undoes_it() {
        let t = Types::parse(TABLE_TEXT);
        let g = t.get("Ghost").unwrap();
        assert_eq!(
            g.apply(),
            vec![
                Step::Movement(Some("Ghost".into())),
                Step::Flying(true),
                Step::NoClip(true),
                Step::Marker(Marker::Invulnerable, true),
                Step::Marker(Marker::Intangible, true),
                Step::Marker(Marker::PreventEmotes, true),
                Step::TriggerBlocks(false),
                Step::Effect(7, true),
                Step::Run("Go".into()),
            ]
        );
        assert_eq!(
            g.unapply(false),
            vec![
                Step::Movement(None),
                Step::Flying(false),
                Step::NoClip(false),
                Step::Marker(Marker::Invulnerable, false),
                Step::Marker(Marker::Intangible, false),
                Step::Marker(Marker::PreventEmotes, false),
                Step::TriggerBlocks(true),
                Step::Effect(7, false),
            ]
        );
        // Back in creative: flying stays, the player stays invulnerable.
        let u = g.unapply(true);
        assert!(!u.contains(&Step::Flying(false)));
        assert!(u.contains(&Step::Marker(Marker::Invulnerable, true)));
    }
}
