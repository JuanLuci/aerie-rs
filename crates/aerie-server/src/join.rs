//! Joining the world after setup (spec 05).
//!
//! The order is the official server's: packets before `JoinWorld` on the game
//! stream, then the two server-opened streams (Chunks, WorldMap), `JoinWorld`,
//! and the world once the client says it is ready for chunks.
//!
//! What the world holds comes from one of two places:
//! - **replay** (test only): everything the official server sent after setup
//!   in a `probe` capture, each packet on the stream its channel names;
//! - **built**: Aerie's own minimal world. The block data inside `SetChunk`
//!   is not decoded yet, so sections are sent without data (empty).

use std::collections::BTreeMap;
use std::time::Duration;

use aerie_protocol::packets::Outgoing;
use aerie_protocol::packets::WorldLoadFinished;
use aerie_protocol::proto;
use aerie_protocol::table::{self, Channel};
use aerie_protocol::wire::Wire;
use aerie_protocol::{Frame, frame};
use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::chunk;
use crate::login::{Game, Player, Settings};
use crate::world::{self, Hub};

/// Packet ids used here (gen/packets.tsv).
mod id {
    pub const DISCONNECT: u32 = 1;
    pub const SERVER_DISCONNECT: u32 = 2;
    pub const PING: u32 = 3;
    pub const PONG: u32 = 4;
    pub const NETWORK_TICK: u32 = 5;
    pub const SET_CLIENT_ID: u32 = 100;
    pub const SET_GAME_MODE: u32 = 101;
    pub const JOIN_WORLD: u32 = 104;
    pub const CLIENT_READY: u32 = 105;
    pub const SET_CHUNK: u32 = 131;
    pub const VIEW_RADIUS: u32 = 32;
    pub const ENTITY_UPDATES: u32 = 161;
    pub const SET_CHUNK_ENVIRONMENTS: u32 = 134;
    pub const SET_FLUIDS: u32 = 136;
    pub const SET_COLUMN: u32 = 138;
    pub const ADD_TO_PLAYER_LIST: u32 = 224;
    pub const PLAYER_LIST_PING: u32 = 227;
    pub const CLEAR_WORLD_MAP: u32 = 242;
    pub const UPDATE_ENTITY_STAT_TYPES: u32 = 72;
    pub const STREAM_OPEN: u32 = 460;
    pub const STREAM_OPEN_RESPONSE: u32 = 461;
}

/// How long to wait for `ClientReady` before sending the world anyway.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Built world: a square of empty columns around the spawn.
/// The client waits on joining until the chunks around it arrive (about
/// 30 s otherwise): it asks for a view radius of 224 blocks, 7 chunks.
const BUILT_RADIUS: i32 = 7;
/// The column a position is in.
fn column_of(p: (f64, f64, f64)) -> (i32, i32) {
    ((p.0 / 32.0).floor() as i32, (p.2 / 32.0).floor() as i32)
}

/// Columns sent per half second as a player moves into new ground.
const STREAM_PER_ROUND: usize = 8;
const BUILT_SECTIONS: i32 = 10;
/// The built world's terrain, bottom up: (blocks tried in order, layers).
/// Without block ids (no `Assets.zip`) the world stays empty.
const TERRAIN: &[(&[&str], usize)] = &[
    (&["Rock_Bedrock", "Rock_Stone"], 1),
    (&["Rock_Stone"], 56),
    (&["Soil_Dirt", "Rock_Stone"], 6),
    (&["Soil_Grass", "Soil_Dirt", "Rock_Stone"], 1),
];

/// Block ids of the terrain layers, bottom up; empty without block ids.
pub fn terrain_layers(block_id: impl Fn(&str) -> Option<i32>) -> Vec<i32> {
    let mut out = Vec::new();
    for (names, n) in TERRAIN {
        let Some(b) = names.iter().find_map(|k| block_id(k)) else {
            return Vec::new();
        };
        out.extend(std::iter::repeat_n(b, *n));
    }
    out
}

/// Name of the player's model among the things built from the assets.
pub const PLAYER_MODEL: &str = "player_model";
/// `WorldGameplayConfig` and `UpdateTimeSettings` built from the gameplay
/// configuration asset.
pub const WORLD_GAMEPLAY: &str = "world_gameplay_config";
pub const TIME_SETTINGS: &str = "time_settings";
/// `UpdateMovementSettings` from the movement configuration asset.
pub const MOVEMENT_SETTINGS: &str = "movement_settings";

/// Packet ids of the world settings sent with the player.
pub(crate) mod settings_id {
    pub const NETWORK_TICK: u32 = 5;
    pub const UPDATE_MOVEMENT_SETTINGS: u32 = 110;
    pub const SET_UPDATE_RATE: u32 = 29;
    pub const SET_TIME_DILATION: u32 = 30;
    pub const SERVER_TAGS: u32 = 34;
    pub const UPDATE_FEATURES: u32 = 31;
    pub const SET_MOVEMENT_STATES: u32 = 102;
    pub const UPDATE_TIME_SETTINGS: u32 = 145;
    pub const UPDATE_TIME: u32 = 146;
    pub const SET_ENTITY_SEED: u32 = 160;
    pub const WORLD_GAMEPLAY_CONFIG: u32 = 185;
    pub const UPDATE_KNOWN_RECIPES: u32 = 228;
    pub const UPDATE_VISIBLE_HUD: u32 = 230;
    pub const UPDATE_SUN_SETTINGS: u32 = 360;
    pub const UPDATE_POST_FX: u32 = 361;
    pub const SET_BLOCK_PLACEMENT_OVERRIDE: u32 = 103;
    pub const MEMORIES_STATUS: u32 = 118;
    pub const MEMORIES_COUNT: u32 = 120;
    pub const ARG_CACHE_INVALIDATION: u32 = 248;
    pub const WORLD_MAP_SETTINGS: u32 = 240;
    pub const VOICE_CONFIG: u32 = 452;
    pub const UPDATE_CLIENT_RULE_VOLUMES: u32 = 473;
}

/// What the player's own entity is built from: the `Player` model (from the
/// assets) and the initial value of each entity stat type (from the built
/// `UpdateEntityStatTypes` registry).
#[derive(Clone, Default)]
pub struct PlayerParts {
    model: Option<proto::Model>,
    stats: Vec<(i32, f32)>,
    /// Encoded `WorldGameplayConfig` and `UpdateTimeSettings`, when built.
    gameplay: Option<Vec<u8>>,
    time: Option<Vec<u8>>,
    movement: Option<Vec<u8>>,
}

impl PlayerParts {
    pub fn new(registries: &[(u32, Vec<u8>)], extras: &[(String, Vec<u8>)]) -> Self {
        let model = extras
            .iter()
            .find(|(n, _)| n == PLAYER_MODEL)
            .and_then(|(_, b)| match proto::Model::decode(b, 0) {
                Ok((m, _)) => Some(m),
                Err(e) => {
                    warn!("the cached Player model does not decode: {e}");
                    None
                }
            });
        // An entity's model lists its animation sets in the official
        // server's map order (spec 17).
        let model = model.map(|mut m: proto::Model| {
            if let Some(sets) = m.animation_sets.take() {
                let keys: Vec<String> = sets.iter().map(|(k, _)| k.clone()).collect();
                let mut sets: Vec<Option<_>> = sets.into_iter().map(Some).collect();
                m.animation_sets = Some(
                    crate::changes::string_map_order(&keys)
                        .into_iter()
                        .filter_map(|i| sets[i].take())
                        .collect(),
                );
            }
            m
        });
        let stats = registries
            .iter()
            .find(|(i, _)| *i == id::UPDATE_ENTITY_STAT_TYPES)
            .and_then(|(_, b)| proto::UpdateEntityStatTypes::decode(b, 0).ok())
            .and_then(|(u, _)| u.types)
            .map(|t| t.into_iter().map(|(i, s)| (i, s.value)).collect())
            .unwrap_or_default();
        let extra = |name: &str| {
            extras
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, b)| b.clone())
        };
        Self {
            model,
            stats,
            gameplay: extra(WORLD_GAMEPLAY),
            time: extra(TIME_SETTINGS),
            movement: extra(MOVEMENT_SETTINGS),
        }
    }
}

/// World settings before the player's entity, in the official order. The
/// gameplay configuration comes from the assets; the rest are Aerie's own
/// choices: normal time, every client feature on, sun overhead, 30 updates
/// per second, not flying.
fn world_settings(parts: &PlayerParts) -> Result<Vec<Out>> {
    let mut v = vec![
        Out::new(
            settings_id::SET_TIME_DILATION,
            &proto::SetTimeDilation { time_dilation: 1.0 },
        )?,
        Out::new(
            settings_id::UPDATE_FEATURES,
            &proto::UpdateFeatures {
                features: Some((0..5).map(|f| (proto::ClientFeature(f), true)).collect()),
            },
        )?,
        Out::new(
            settings_id::UPDATE_SUN_SETTINGS,
            &proto::UpdateSunSettings {
                height_percentage: 100.0,
                angle_radians: 0.0,
            },
        )?,
        Out::new(
            settings_id::UPDATE_POST_FX,
            &proto::UpdatePostFxSettings {
                global_intensity: 0.3,
                power: 8.0,
                sunshaft_scale: 4.0,
                sun_intensity: 0.25,
                sunshaft_intensity: 0.3,
            },
        )?,
    ];
    if let Some(b) = &parts.gameplay {
        v.push(Out {
            id: settings_id::WORLD_GAMEPLAY_CONFIG,
            payload: b.clone(),
        });
    }
    v.push(Out::new(
        settings_id::SET_UPDATE_RATE,
        &proto::SetUpdateRate {
            updates_per_second: 30,
        },
    )?);
    v.push(Out::new(
        settings_id::SET_MOVEMENT_STATES,
        &proto::SetMovementStates {
            movement_states: Some(proto::SavedMovementStates { flying: false }),
        },
    )?);
    Ok(v)
}

/// The client tags the server announces before `JoinWorld`. The movement
/// abilities (crouch slide, mantling, safety roll) each register
/// "Allows=Movement"; as the first tag ever registered it has index 0. The
/// five client features themselves go out later, in `world_settings`.
fn client_tags() -> Result<Vec<Out>> {
    Ok(vec![Out::new(
        settings_id::SERVER_TAGS,
        &proto::ServerTags {
            tags: Some(vec![("Allows=Movement".to_string(), 0)]),
        },
    )?])
}

/// What follows the join message: the command argument types whose cached
/// answers the client must drop (the list of players changed), and the world
/// map settings, sent twice.
fn map_and_args() -> Result<Vec<Out>> {
    let settings = || {
        Out::new(
            settings_id::WORLD_MAP_SETTINGS,
            &proto::UpdateWorldMapSettings {
                enabled: true,
                allow_teleport_to_coordinates: false,
                allow_teleport_to_markers: false,
                allow_show_on_map_toggle: true,
                allow_compass_tracking_toggle: true,
                allow_creating_map_markers: true,
                allow_removing_other_players_markers: true,
                default_scale: 128.0,
                min_scale: 32.0,
                max_scale: 175.0,
            },
        )
    };
    Ok(vec![
        Out::new(
            settings_id::ARG_CACHE_INVALIDATION,
            &proto::ArgCacheInvalidation {
                arg_type_ids: Some(vec!["player_ref".to_string()]),
            },
        )?,
        settings()?,
        settings()?,
    ])
}

/// After the player: the HUD, known recipes, time settings and the time
/// (noon of the first day), then a network tick.
fn hud_and_time(parts: &PlayerParts) -> Result<Vec<Out>> {
    use proto::HudComponent as H;
    // Everything a player normally sees; the player list, pending requests
    // and the builder tools' material slot appear on demand.
    let hidden = [
        H::REQUESTS,
        H::PLAYER_LIST,
        H::BUILDER_TOOLS_MATERIAL_SLOT_SELECTOR,
    ];
    let visible = (0..=25).map(H).filter(|h| !hidden.contains(h)).collect();
    // Block placement restrictions kept, then the memories feature locked
    // with nothing collected.
    let mut v = vec![
        Out::new(
            settings_id::SET_BLOCK_PLACEMENT_OVERRIDE,
            &proto::SetBlockPlacementOverride { enabled: false },
        )?,
        Out::new(
            settings_id::MEMORIES_STATUS,
            &proto::UpdateMemoriesFeatureStatus {
                is_feature_unlocked: false,
            },
        )?,
        Out::new(
            settings_id::MEMORIES_COUNT,
            &proto::UpdateMemoriesCount { count: 0 },
        )?,
    ];
    if let Some(b) = &parts.movement {
        v.push(Out {
            id: settings_id::UPDATE_MOVEMENT_SETTINGS,
            payload: b.clone(),
        });
    }
    v.extend([
        Out::new(
            settings_id::UPDATE_VISIBLE_HUD,
            &proto::UpdateVisibleHudComponents {
                visible_components: Some(visible),
            },
        )?,
        Out::new(
            settings_id::UPDATE_KNOWN_RECIPES,
            &proto::UpdateKnownRecipes {
                known: Some(Vec::new()),
            },
        )?,
    ]);
    if let Some(b) = &parts.time {
        v.push(Out {
            id: settings_id::UPDATE_TIME_SETTINGS,
            payload: b.clone(),
        });
    }
    // Game time counts from 0001-01-01 00:00 (the captured clock reads in
    // those seconds): noon of the first day.
    const YEAR_ONE: i64 = -62_135_596_800;
    v.push(Out::new(
        settings_id::UPDATE_TIME,
        &proto::UpdateTime {
            game_time: Some(proto::InstantData {
                seconds: YEAR_ONE + 12 * 3600,
                nanos: 0,
            }),
        },
    )?);
    v.push(Out::new(
        settings_id::NETWORK_TICK,
        &proto::NetworkTick {
            tick_duration_micros: 1_000_000 / 30,
            server_tick: 0,
        },
    )?);
    Ok(v)
}

/// The player's own entity, as the official server sends it after
/// `JoinWorld`: model, no skin, empty equipment, where it stands, no
/// effects, every stat at its initial value.
/// How other players see this one: its own entity plus a name plate, the
/// default hitbox collision, the UI components (health bar, name) and
/// movement states, in the official server's component order.
fn seen_by_others(
    client_id: i32,
    parts: &PlayerParts,
    name: &str,
    at: (f64, f64, f64),
) -> Result<proto::EntityUpdate> {
    use proto::ComponentUpdate as C;
    let own = player_entity(client_id, parts, at)?;
    let (mut u, _) = proto::EntityUpdates::decode(&own.payload, 0)?;
    let mut e = u
        .updates
        .take()
        .and_then(|mut v| v.pop())
        .unwrap_or_default();
    let mut out = Vec::new();
    for c in e.updates.take().unwrap_or_default() {
        let after_model = matches!(c, C::ModelUpdate(_));
        let after_equipment = matches!(c, C::EquipmentUpdate(_));
        out.push(c);
        if after_model {
            out.push(C::NameplateUpdate(Box::new(proto::NameplateUpdate {
                text: name.to_owned(),
            })));
            out.push(C::HitboxCollisionUpdate(Box::new(
                proto::HitboxCollisionUpdate {
                    hitbox_collision_config_index: 0,
                },
            )));
        }
        if after_equipment {
            out.push(C::UIComponentsUpdate(Box::new(proto::UIComponentsUpdate {
                components: vec![0, 1],
            })));
            out.push(C::MovementStatesUpdate(Box::new(
                proto::MovementStatesUpdate::default(),
            )));
        }
    }
    e.updates = Some(out);
    Ok(e)
}

pub fn player_entity_packet_for_check(client_id: i32, parts: &PlayerParts) -> Result<Vec<u8>> {
    Ok(player_entity(client_id, parts, SPAWN)?.payload)
}

fn player_entity(client_id: i32, parts: &PlayerParts, at: (f64, f64, f64)) -> Result<Out> {
    use proto::ComponentUpdate as C;
    let facing = proto::Direction {
        yaw: 0.0,
        pitch: 0.0,
        roll: 0.0,
    };
    let stats = parts
        .stats
        .iter()
        .map(|&(i, value)| {
            (
                i,
                vec![proto::EntityStatUpdate {
                    op: proto::EntityStatOp(0),
                    predictable: false,
                    value,
                    modifiers: None,
                    modifier_key: None,
                    modifier: None,
                }],
            )
        })
        .collect();
    let updates = vec![
        C::ModelUpdate(Box::new(proto::ModelUpdate {
            model: parts.model.clone(),
            entity_scale: 0.0,
        })),
        C::PlayerSkinUpdate(Box::new(proto::PlayerSkinUpdate { skin: None })),
        C::EquipmentUpdate(Box::new(proto::EquipmentUpdate {
            armor_ids: Some(vec![String::new(); 4]),
            right_hand_item_id: Some("Empty".into()),
            left_hand_item_id: Some("Empty".into()),
        })),
        C::TransformUpdate(Box::new(proto::TransformUpdate {
            transform: proto::ModelTransform {
                position: Some(proto::Position {
                    x: at.0,
                    y: at.1,
                    z: at.2,
                }),
                body_orientation: Some(facing.clone()),
                look_orientation: Some(facing),
            },
        })),
        C::EntityEffectsUpdate(Box::new(proto::EntityEffectsUpdate {
            entity_effect_updates: Vec::new(),
        })),
        C::EntityStatsUpdate(Box::new(proto::EntityStatsUpdate {
            entity_stat_updates: stats,
        })),
    ];
    Out::new(
        id::ENTITY_UPDATES,
        &proto::EntityUpdates {
            removed: None,
            updates: Some(vec![proto::EntityUpdate {
                network_id: client_id,
                removed: None,
                updates: Some(updates),
            }]),
        },
    )
}

/// The world Aerie builds.
pub const WORLD_UUID: uuid::Uuid = uuid::Uuid::from_u128(0xae1e_0000_0000_4000_8000_0000_0000_0001);
/// Above the terrain (64 layers) or, without it, in the air.
const SPAWN: (f64, f64, f64) = (16.0, 66.0, 16.0);

/// One packet to send, already serialized.
struct Out {
    id: u32,
    payload: Vec<u8>,
}

impl Out {
    fn new<T: Wire>(id: u32, p: &T) -> Result<Self> {
        let mut payload = Vec::new();
        p.encode(&mut payload)?;
        Ok(Self { id, payload })
    }
    fn channel(&self) -> Channel {
        table::info(self.id).map_or(Channel::Default, |i| i.channel)
    }
}

/// What goes out before the client answers `JoinWorld`: the game stream
/// packets that precede it, and `JoinWorld` itself. Cheap to make, so it can
/// leave at once while the rest of the world is still being built.
struct Head {
    before: Vec<Out>,
    /// After the player list, still before `JoinWorld`.
    late: Vec<Out>,
    join: Out,
}

/// What the server sends to put the player in the world, once the client is
/// ready for chunks.
struct World {
    /// The player's own entity and state on the game stream, in the order
    /// the official server sends them (spec 05).
    player: Vec<Out>,
    /// Sent after the join message and the world map clear, before the
    /// network tick.
    map: Vec<Out>,
    /// Everything sent once the client is ready for chunks, in order, on
    /// every stream.
    after: Vec<Out>,
}

/// The part of a `probe` capture that follows setup: everything after
/// `WorldLoadFinished`, split around `JoinWorld`. `None` if the capture has
/// no join.
fn from_capture(capture: &[(u32, Vec<u8>)]) -> Option<(Head, World)> {
    let start = capture
        .iter()
        .position(|(i, _)| *i == WorldLoadFinished::ID)?
        + 1;
    let rest = &capture[start..];
    let j = rest.iter().position(|(i, _)| *i == id::JOIN_WORLD)?;
    // Pings are the server's own (sent live, with live times); a captured
    // disconnect is the end of the probe's visit, not part of the world.
    let keep = |(i, _): &&(u32, Vec<u8>)| !matches!(*i, id::PING | id::SERVER_DISCONNECT);
    let out = |(i, p): &(u32, Vec<u8>)| Out {
        id: *i,
        payload: p.clone(),
    };
    Some((
        Head {
            before: rest[..j].iter().filter(keep).map(out).collect(),
            late: Vec::new(),
            join: out(&rest[j]),
        },
        World {
            player: Vec::new(),
            map: Vec::new(),
            after: rest[j + 1..].iter().filter(keep).map(out).collect(),
        },
    ))
}

/// Test only: the player packets of a capture (game stream, from
/// `JoinWorld` to the first `SetColumn`), for a built world that cannot make
/// the player's entity yet. The player is moved to the built spawn.
fn captured_player(capture: &[(u32, Vec<u8>)]) -> Option<(Vec<Out>, Vec<Out>)> {
    let (head, w) = from_capture(capture)?;
    let player_list = |p: &Out| matches!(p.id, id::ADD_TO_PLAYER_LIST | id::PLAYER_LIST_PING);
    let before = head
        .before
        .into_iter()
        .filter(|p| !player_list(p))
        .collect();
    let mut player = Vec::new();
    for p in w.after {
        if p.id == id::SET_COLUMN {
            break;
        }
        if p.channel() != Channel::Default || player_list(&p) {
            continue;
        }
        player.push(if p.id == id::ENTITY_UPDATES {
            at_spawn(p).ok()?
        } else {
            p
        });
    }
    Some((before, player))
}

/// Test only (`--captured-player IDS`): the built player plus the captured
/// player packets whose ids are listed (`all` for every one), to find out
/// one by one which ones the client needs. The captured entity, client id
/// and game mode are always the built ones.
fn built_player_with(capture: &[(u32, Vec<u8>)], keep: &str) -> Option<(Vec<Out>, Vec<Out>)> {
    let (before, player) = captured_player(capture)?;
    let all = keep.split(',').any(|k| k.trim() == "all");
    let ids: Vec<u32> = keep
        .split(',')
        .filter_map(|k| k.trim().parse().ok())
        .collect();
    let own = [id::ENTITY_UPDATES, id::SET_CLIENT_ID, id::SET_GAME_MODE];
    let player = player
        .into_iter()
        .filter(|p| !own.contains(&p.id) && (all || ids.contains(&p.id)))
        .collect();
    Some((before, player))
}

/// Puts every transform of an `EntityUpdates` at the built spawn.
fn at_spawn(p: Out) -> Result<Out> {
    let (mut u, _) = proto::EntityUpdates::decode(&p.payload, 0)?;
    for e in u.updates.iter_mut().flatten() {
        for c in e.updates.iter_mut().flatten() {
            if let proto::ComponentUpdate::TransformUpdate(t) = c {
                t.transform.position = Some(proto::Position {
                    x: SPAWN.0,
                    y: SPAWN.1,
                    z: SPAWN.2,
                });
            }
        }
    }
    Out::new(id::ENTITY_UPDATES, &u)
}

/// A uniform tint for a column. `SetColumn.tintmap` is an i16 number of
/// colours, the RGBA colours, an i32 bitfield length (always 1280: 1024
/// columns × 10 bits) and the bitfield of colour indices (spec 08).
fn uniform_tint(rgba: [u8; 4]) -> Vec<i8> {
    let mut t = Vec::with_capacity(2 + 4 + 4 + 1280);
    t.extend_from_slice(&1i16.to_le_bytes());
    t.extend_from_slice(&rgba);
    t.extend_from_slice(&1280i32.to_le_bytes());
    t.resize(t.len() + 1280, 0);
    t.into_iter().map(|b| b as i8).collect()
}

/// Grass green, as the captured columns' tints.
const GRASS_TINT: [u8; 4] = [0x28, 0x95, 0x5b, 0xff];

/// Block changes inside a section: (x, y, z within it, block id).
pub type Overlay<'a> = &'a dyn Fn(i32, i32, i32) -> Vec<(usize, usize, usize, i32, i32, i32)>;

/// One column of the built world, as the official server sends it: its
/// `SetColumn`, then each section with its environment and fluids. The
/// overlay puts changed blocks into the sections.
fn column(
    cx: i32,
    cz: i32,
    layers: &[i32],
    generator: Option<&crate::terrain::Generator>,
    overlay: Overlay,
    opaque: &dyn Fn(i32) -> bool,
) -> Result<Vec<Out>> {
    let mut sections = Vec::new();
    for cy in 0..BUILT_SECTIONS {
        let mut section = match generator {
            Some(g) => g.section(cx, cy, cz),
            None => chunk::flat(cy, layers),
        };
        for (x, y, z, b, r, f) in overlay(cx, cy, cz) {
            section.set(x, y, z, b);
            section.rotations[crate::chunk::index(x, y, z)] = r;
            section.fillers[crate::chunk::index(x, y, z)] = f;
        }
        sections.push(section);
    }
    // The highest block that is not transparent in each column (index
    // z * 32 + x; none: i32::MIN), as the official heightmap (spec 10).
    let mut heightmap = vec![i32::MIN; 1024];
    for z in 0..chunk::SIZE {
        for x in 0..chunk::SIZE {
            'col: for (cy, s) in sections.iter().enumerate().rev() {
                for y in (0..chunk::SIZE).rev() {
                    let b = s.blocks[chunk::index(x, y, z)];
                    if b != 0 && opaque(b) {
                        heightmap[z * chunk::SIZE + x] = (cy * chunk::SIZE + y) as i32;
                        break 'col;
                    }
                }
            }
        }
    }
    let mut out = vec![Out::new(
        id::SET_COLUMN,
        &proto::SetColumn {
            x: cx,
            z: cz,
            heightmap,
            tintmap: uniform_tint(GRASS_TINT),
        },
    )?];
    for (cy, section) in sections.into_iter().enumerate() {
        let cy = cy as i32;
        let data = section.encode();
        out.push(Out::new(
            id::SET_CHUNK,
            &proto::SetChunk {
                x: cx,
                y: cy,
                z: cz,
                // As the official server sends it (one zero byte).
                local_light: Some(vec![0]),
                global_light: None,
                data: data.map(|d| d.into_iter().map(|b| b as i8).collect()),
            },
        )?);
        // The empty forms the official server sends.
        out.push(Out::new(
            id::SET_CHUNK_ENVIRONMENTS,
            &proto::SetChunkEnvironments {
                x: cx,
                y: cy,
                z: cz,
                environments: vec![0, 0],
            },
        )?);
        out.push(Out::new(
            id::SET_FLUIDS,
            &proto::SetFluids {
                x: cx,
                y: cy,
                z: cz,
                data: Some(vec![0, 0]),
            },
        )?);
    }
    Ok(out)
}

/// Aerie's own world: the server's game mode (adventure unless
/// `--game-mode creative`) on Aerie's terrain (spec 08), sent in the
/// official server's order: per column a `SetColumn`, then each section with
/// its environment and fluids. `player` comes from a capture when one is
/// given (see `captured_player`); otherwise only the client id and game mode.
/// Test only: player packets taken from a capture for the built world.
enum Captured {
    /// The whole captured player (`captured_player`).
    Player(Vec<Out>, Vec<Out>),
    /// Some captured packets added to the built player (`built_player_with`).
    Extras(Vec<Out>, Vec<Out>),
}

/// What precedes the built world's `JoinWorld`: the client tags (or the
/// captured packets, test only) and `JoinWorld`.
fn built_head(captured: &Option<Captured>) -> Result<Head> {
    let before = match captured {
        Some(Captured::Player(before, _) | Captured::Extras(before, _)) => before
            .iter()
            .map(|p| Out {
                id: p.id,
                payload: p.payload.clone(),
            })
            .collect(),
        None => client_tags()?,
    };
    let late = match captured {
        Some(_) => Vec::new(),
        None => vec![
            Out::new(
                settings_id::VOICE_CONFIG,
                &proto::VoiceConfig {
                    voice_enabled: true,
                    codec: proto::VoiceCodec::OPUS,
                    sample_rate: 48000,
                    channels: 1,
                    max_hearing_distance: 32.0,
                    reference_distance: 4.0,
                    supports_voice_stream: true,
                    max_packets_per_second: 60,
                },
            )?,
            Out::new(
                settings_id::UPDATE_CLIENT_RULE_VOLUMES,
                &proto::UpdateClientRuleVolumes {
                    volumes: Some(Vec::new()),
                },
            )?,
        ],
    };
    let join = Out::new(
        id::JOIN_WORLD,
        &proto::JoinWorld {
            clear_world: false,
            fade_in_out: false,
            world_uuid: WORLD_UUID,
            min_section_y: Some(0),
            max_section_y: Some(9),
        },
    )?;
    Ok(Head { before, late, join })
}

fn built(
    client_id: i32,
    parts: &PlayerParts,
    captured: Option<Captured>,
    start: (f64, f64, f64),
    creative: bool,
    view_radius: i32,
) -> Result<World> {
    let own = || -> Result<Vec<Out>> {
        let mut v = vec![
            Out::new(
                settings_id::SET_ENTITY_SEED,
                &proto::SetEntitySeed {
                    entity_seed: rand::random(),
                },
            )?,
            Out::new(id::SET_CLIENT_ID, &proto::SetClientId { client_id })?,
        ];
        v.extend(world_settings(parts)?);
        v.push(player_entity(client_id, parts, start)?);
        v.push(Out::new(
            id::SET_GAME_MODE,
            &proto::SetGameMode {
                game_mode: if creative {
                    proto::GameMode::CREATIVE
                } else {
                    proto::GameMode::ADVENTURE
                },
            },
        )?);
        v.extend(hud_and_time(parts)?);
        Ok(v)
    };
    let (player, map) = match captured {
        Some(Captured::Player(_, player)) => (player, Vec::new()),
        Some(Captured::Extras(_, extras)) => {
            let mut p = own()?;
            p.extend(extras);
            (p, map_and_args()?)
        }
        None => (own()?, map_and_args()?),
    };
    let mut after = vec![Out::new(
        id::VIEW_RADIUS,
        &proto::ViewRadius {
            value: view_radius * 32,
        },
    )?];
    after.push(Out {
        id: id::CLEAR_WORLD_MAP,
        payload: Vec::new(),
    });
    Ok(World { player, map, after })
}

/// The columns of the built world, nearest to where the player starts
/// first.
fn terrain_area(start: (f64, f64, f64)) -> Vec<(i32, i32)> {
    let (ccx, ccz) = column_of(start);
    let mut area: Vec<(i32, i32)> = (ccx - BUILT_RADIUS..=ccx + BUILT_RADIUS)
        .flat_map(|x| (ccz - BUILT_RADIUS..=ccz + BUILT_RADIUS).map(move |z| (x, z)))
        .collect();
    area.sort_by_key(|(x, z)| (x - ccx).pow(2) + (z - ccz).pow(2));
    area
}

/// One column of the built world with the damage of the blocks being broken
/// in it. Making the whole area is the slow part of a join (about a
/// second), so it is made column by column after the player's packets are
/// out, answering the client in between.
fn terrain_column(
    (cx, cz): (i32, i32),
    layers: &[i32],
    generator: Option<&crate::terrain::Generator>,
    overlay: Overlay,
    opaque: &dyn Fn(i32) -> bool,
    damage: &dyn Fn(i32, i32) -> Vec<(u32, Vec<u8>)>,
) -> Result<Vec<Out>> {
    let mut out = column(cx, cz, layers, generator, overlay, opaque)?;
    out.extend(
        damage(cx, cz)
            .into_iter()
            .map(|(id, payload)| Out { id, payload }),
    );
    Ok(out)
}

/// The two streams the server opens before `JoinWorld`, Chunks first. QUIC
/// numbers streams in the order they are opened and the client accepts them
/// in that order, whichever one carries data first.
struct Streams {
    chunks: quinn::SendStream,
    world_map: quinn::SendStream,
}

/// `ClientReady` flags (spec 05): bit 0 chunks, bit 1 gameplay.
#[derive(Default)]
struct Ready {
    chunks: bool,
    gameplay: bool,
    /// Where the spawn teleport goes, until it is sent.
    spawn: Option<proto::Position>,
}

/// Writes one packet on the stream its channel names.
async fn write_out(g: &mut Game, streams: &mut Streams, id: u32, payload: &[u8]) -> Result<()> {
    let mut wire = Vec::new();
    frame::encode(id, payload, &mut wire)?;
    match table::info(id).map_or(Channel::Default, |i| i.channel) {
        Channel::Chunks => streams.chunks.write_all(&wire).await?,
        Channel::WorldMap => streams.world_map.write_all(&wire).await?,
        _ => g.send.write_all(&wire).await?,
    }
    Ok(())
}

/// Sends what the hub has queued for this player so far, in order.
async fn flush_outbox(
    g: &mut Game,
    streams: &mut Streams,
    outbox: &mut tokio::sync::mpsc::UnboundedReceiver<world::Packet>,
) -> Result<()> {
    while let Ok((id, payload)) = outbox.try_recv() {
        write_out(g, streams, id, &payload).await?;
    }
    Ok(())
}

/// Counts client packets by id, so movement does not flood the log, and
/// with `--record DIR` saves the first few of each kind for study (the
/// same layout as a `probe` capture: `index.tsv` and one file per packet).
#[derive(Default)]
struct Seen(BTreeMap<u32, u32>, Option<Recorder>);

struct Recorder {
    dir: std::path::PathBuf,
    seq: u32,
    kept: BTreeMap<u32, u32>,
    start: std::time::Instant,
}

/// Packets of each kind kept by `--record`.
const RECORD_EACH: u32 = 40;

impl Seen {
    fn record(&mut self, f: &Frame) {
        let Some(r) = &mut self.1 else { return };
        let n = r.kept.entry(f.id).or_default();
        // Interactions come in hundreds per action: keep more of them.
        let limit = if f.id == world::id::SYNC_INTERACTION_CHAINS {
            RECORD_EACH * 10
        } else {
            RECORD_EACH
        };
        // Never cut: windows and inventory actions, and interaction chains
        // that start something other than a left click (using a bench,
        // eating, placing), which the official comparison needs.
        // Every interaction chain packet, so a recording can be replayed
        // whole (with its timing) against the official server.
        let always = (172..=176).contains(&f.id)
            || (202..=204).contains(&f.id)
            || f.id == world::id::SYNC_INTERACTION_CHAINS;
        if *n >= limit && !always {
            return;
        }
        *n += 1;
        let name = table::info(f.id).map_or("?", |i| i.name);
        let file = format!("{:04}-{}-{name}.bin", r.seq, f.id);
        let line = format!(
            "{}\t{}\t{name}\t{}\tfrom-client\t{}\n",
            r.seq,
            f.id,
            f.payload.len(),
            r.start.elapsed().as_millis()
        );
        r.seq += 1;
        let ok = std::fs::write(r.dir.join(&file), &f.payload).and_then(|_| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(r.dir.join("index.tsv"))?
                .write_all(line.as_bytes())
        });
        if let Err(e) = ok {
            warn!("recording a packet failed: {e}");
            self.1 = None;
        }
    }

    fn add(&mut self, id: u32) {
        *self.0.entry(id).or_default() += 1;
    }
    fn flush(&mut self) {
        if self.0.is_empty() {
            return;
        }
        let s: Vec<String> = self
            .0
            .iter()
            .map(|(i, n)| format!("{} ×{n}", table::info(*i).map_or("?", |x| x.name)))
            .collect();
        info!("← {}", s.join(", "));
        self.0.clear();
    }
}

pub async fn run(
    g: &mut Game,
    conn: &quinn::Connection,
    settings: &Settings,
    player: &Player,
) -> Result<()> {
    let (client_id, mut outbox) = settings.hub.join(&player.username, player.uuid);
    let streams = tokio::spawn(client_streams(conn.clone(), player.username.clone()));
    let result = in_world(g, conn, settings, player, client_id, &mut outbox).await;
    streams.abort();
    settings.hub.leave(client_id);
    result
}

/// Streams the client opens itself (the official server's
/// `PendingStreamHandler`): the first packet is a `StreamOpen` naming the
/// stream type, answered on the same stream with a `StreamOpenResponse`.
/// Only Voice exists besides Game (stream 0, never opened this way). Voice is
/// accepted and what comes on it is read and dropped; without an answer the
/// client waits 5 s and retries 5 times, which delays loading.
async fn client_streams(conn: quinn::Connection, name: String) {
    while let Ok((send, recv)) = conn.accept_bi().await {
        let name = name.clone();
        tokio::spawn(async move {
            if let Err(e) = client_stream(send, recv).await {
                debug!("{name}: client stream ended: {e:#}");
            }
        });
    }
}

async fn client_stream(mut send: quinn::SendStream, mut recv: quinn::RecvStream) -> Result<()> {
    let mut dec = aerie_protocol::frame::FrameDecoder::new(true);
    let mut buf = vec![0u8; 16 * 1024];
    let mut opened = false;
    loop {
        while let Some(f) = dec.next()? {
            if opened {
                // VoiceData: not routed (no voice chat yet).
                continue;
            }
            anyhow::ensure!(
                f.id == id::STREAM_OPEN,
                "first packet {} is not StreamOpen",
                f.id
            );
            let (open, _) = proto::StreamOpen::decode(&f.payload, 0)?;
            let accepted = open.r#type == proto::StreamType::VOICE;
            let reply = proto::StreamOpenResponse {
                r#type: open.r#type,
                accepted,
                rejection_reason: (!accepted).then(|| "Stream type not supported".to_string()),
            };
            let mut payload = Vec::new();
            reply.encode(&mut payload)?;
            let mut wire = Vec::new();
            frame::encode(id::STREAM_OPEN_RESPONSE, &payload, &mut wire)?;
            send.write_all(&wire).await?;
            info!(
                "client stream {:?}: {}",
                open.r#type,
                if accepted { "accepted" } else { "refused" }
            );
            if !accepted {
                let _ = send.finish();
                return Ok(());
            }
            opened = true;
        }
        let Some(n) = recv.read(&mut buf).await? else {
            return Ok(());
        };
        dec.push(&buf[..n]);
    }
}

async fn in_world(
    g: &mut Game,
    conn: &quinn::Connection,
    settings: &Settings,
    player: &Player,
    client_id: i32,
    outbox: &mut tokio::sync::mpsc::UnboundedReceiver<world::Packet>,
) -> Result<()> {
    let hub = &settings.hub;
    let replay = if settings.built_world {
        None
    } else {
        settings.replay.as_deref().and_then(from_capture)
    };
    let layers = terrain_layers(|k| hub.block_id(k));
    // Generated terrain unless `--flat`.
    let generator = (!settings.flat)
        .then(|| crate::terrain::Palette::new(|k| hub.block_id(k)))
        .flatten()
        .map(|palette| crate::terrain::Generator {
            seed: settings.seed,
            palette,
        });
    // Where the player starts: where it left, else the spawn.
    let start = hub
        .saved_position(player.uuid)
        .filter(|p| p.y > 0.0)
        .map_or(SPAWN, |p| (p.x, p.y, p.z));
    let overlay = |x, y, z| hub.changes_in(x, y, z);
    hub.set_ground(layers.clone(), generator.clone());
    let mut pending = None;
    let (head, mut world, replayed) = match replay {
        Some((head, world)) => {
            info!(
                "join: replaying the captured world ({} + {} packets, test only)",
                head.before.len() + 1,
                world.after.len()
            );
            (head, Some(world), true)
        }
        None => {
            if layers.is_empty() {
                warn!("no block ids (no Assets.zip?): the built world has no terrain");
            }
            let captured =
                settings
                    .replay
                    .as_deref()
                    .and_then(|c| match &settings.captured_player {
                        Some(keep) => {
                            info!("join: built player with captured packets {keep} (test only)");
                            built_player_with(c, keep).map(|(b, p)| Captured::Extras(b, p))
                        }
                        None => {
                            info!("join: built world with the captured player (test only)");
                            captured_player(c).map(|(b, p)| Captured::Player(b, p))
                        }
                    });
            let head = built_head(&captured)?;
            pending = Some(captured);
            (head, None, false)
        }
    };

    // The official server sends the packets before `JoinWorld` and
    // `JoinWorld` at once; the world follows only when the client is ready
    // for chunks (spec 05).
    for p in &head.before {
        g.send_raw(p.id, &p.payload).await?;
    }
    if !replayed {
        // The player list the hub queued at connect comes right after the
        // tags (spec 27).
        while let Ok((id, payload)) = outbox.try_recv() {
            g.send_raw(id, &payload).await?;
        }
    }
    for p in &head.late {
        g.send_raw(p.id, &p.payload).await?;
    }
    let mut streams = Streams {
        chunks: conn.open_uni().await.context("opening the Chunks stream")?,
        world_map: conn
            .open_uni()
            .await
            .context("opening the WorldMap stream")?,
    };
    g.send_raw(head.join.id, &head.join.payload).await?;
    info!("→ JoinWorld to {}", player.username);
    let joined_at = std::time::Instant::now();
    if replayed {
        hub.announce_join(client_id);
    }
    let world = match world.take() {
        Some(w) => w,
        None => {
            let captured = pending.take().flatten();
            let w = built(
                client_id,
                &{
                    // The player's own stats as saved (spec 17).
                    let mut p = settings.player.clone();
                    if let Some(s) = hub.stat_values(client_id) {
                        p.stats = s;
                    }
                    p
                },
                captured,
                start,
                hub.default_creative(),
                settings.max_view_radius,
            )?;
            w
        }
    };

    let mut ready = Ready::default();
    let mut seen = Seen::default();
    if let Some(dir) = &settings.record {
        let dir = dir.join(&player.username);
        match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                info!("recording client packets into {}", dir.display());
                seen.1 = Some(Recorder {
                    dir,
                    seq: 0,
                    kept: BTreeMap::new(),
                    start: std::time::Instant::now(),
                });
            }
            Err(e) => warn!("cannot record into {}: {e}", dir.display()),
        }
    }
    // The player and the chunks go once the client is ready for chunks, as
    // the official server does (it waits up to 30 s).
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    while !ready.chunks {
        let f = match tokio::time::timeout_at(deadline, g.next()).await {
            Ok(f) => f?,
            Err(_) => {
                warn!("no ClientReady after {READY_TIMEOUT:?}: sending the world anyway");
                break;
            }
        };
        if !client_packet(&f, hub, client_id, &mut ready, &mut seen) {
            return Ok(());
        }
    }

    let mut sent = [0usize; 4];
    let mut after = world.after;
    if replayed {
        for p in &after {
            write_out(g, &mut streams, p.id, &p.payload).await?;
            sent[p.channel() as usize] += 1;
        }
        after.clear();
        hub.send_inventory(client_id)?;
        hub.send_active_slots(client_id)?;
        hub.enter_world(
            client_id,
            seen_by_others(client_id, &settings.player, &player.username, start)?,
        )?;
    } else {
        // The view radius first, then the player up to its game mode, its
        // inventory and active slots, the rest of the settings and the
        // time, the join message, the world map, a network tick; the
        // chunks follow (spec 05).
        let mut chunks = after.into_iter();
        if let Some(v) = chunks.next() {
            write_out(g, &mut streams, v.id, &v.payload).await?;
        }
        let mut player_packets = world.player;
        let split = player_packets
            .iter()
            .position(|p| p.id == id::SET_GAME_MODE)
            .map_or(player_packets.len(), |i| i + 1);
        let rest = player_packets.split_off(split);
        for p in &player_packets {
            write_out(g, &mut streams, p.id, &p.payload).await?;
        }
        hub.send_inventory(client_id)?;
        hub.send_active_slots(client_id)?;
        flush_outbox(g, &mut streams, outbox).await?;
        let mut rest: Vec<Out> = rest;
        let tick = rest.pop_if(|p| p.id == id::NETWORK_TICK);
        for p in &rest {
            write_out(g, &mut streams, p.id, &p.payload).await?;
        }
        hub.announce_join(client_id);
        flush_outbox(g, &mut streams, outbox).await?;
        for clear in chunks {
            write_out(g, &mut streams, clear.id, &clear.payload).await?;
        }
        for p in &world.map {
            write_out(g, &mut streams, p.id, &p.payload).await?;
            if p.id == settings_id::ARG_CACHE_INVALIDATION {
                // The official server tells everyone, once the player is in.
                hub.send_others(client_id, &(p.id, p.payload.clone()));
            }
        }
        if let Some(t) = tick {
            write_out(g, &mut streams, t.id, &t.payload).await?;
        }
        hub.send_changed_inventory(client_id)?;
        hub.enter_world(
            client_id,
            seen_by_others(client_id, &settings.player, &player.username, start)?,
        )?;
        // The spawn teleport goes out when the client says it is ready for
        // gameplay: now if it already did, else with that packet.
        ready.spawn = Some(proto::Position {
            x: start.0,
            y: start.1,
            z: start.2,
        });
        if ready.gameplay
            && let Some(at) = ready.spawn.take()
        {
            hub.teleport(client_id, at)?;
        }
        // The terrain last: it takes the longest to make. The client is
        // answered between columns.
        let started = std::time::Instant::now();
        let mut build_t = Duration::ZERO;
        for at in terrain_area(start) {
            while let Ok(f) = tokio::time::timeout(Duration::ZERO, g.next()).await {
                if !client_packet(&f?, hub, client_id, &mut ready, &mut seen) {
                    return Ok(());
                }
            }
            let t0 = std::time::Instant::now();
            let out = terrain_column(
                at,
                &layers,
                generator.as_ref(),
                &overlay,
                &|b| hub.opaque(b),
                &|x, z| hub.damage_packets(x, z),
            )?;
            build_t += t0.elapsed();
            for p in &out {
                write_out(g, &mut streams, p.id, &p.payload).await?;
                sent[p.channel() as usize] += 1;
            }
            flush_outbox(g, &mut streams, outbox).await?;
        }
        info!(
            "terrain sent in {:?} (making it {:?})",
            started.elapsed(),
            build_t
        );
    }
    info!(
        "world sent: {} game, {} chunk and {} map packets (client ready after {:?})",
        sent[0],
        sent[1],
        sent[2],
        joined_at.elapsed()
    );

    // In the world: ping, deliver what other players cause, and read the client.
    let mut report = tokio::time::interval(Duration::from_secs(10));
    // Columns this client has: the built area at first; more as it moves.
    let mut columns: std::collections::HashSet<(i32, i32)> = if replayed {
        Default::default()
    } else {
        let (ccx, ccz) = column_of(start);
        (ccx - BUILT_RADIUS..=ccx + BUILT_RADIUS)
            .flat_map(|x| (ccz - BUILT_RADIUS..=ccz + BUILT_RADIUS).map(move |z| (x, z)))
            .collect()
    };
    settings
        .hub
        .columns_sent(client_id, columns.iter().copied());
    let mut stream_tick = tokio::time::interval(Duration::from_millis(500));
    let mut last_pos = None;
    loop {
        tokio::select! {
            f = g.next() => {
                if !client_packet(&f?, hub, client_id, &mut ready, &mut seen) {
                    break;
                }
            }
            Some((id, payload)) = outbox.recv() => {
                write_out(g, &mut streams, id, &payload).await?;
            }
            _ = stream_tick.tick(), if !replayed => {
                // Columns around the player that it does not have yet, the
                // nearest first, a few per round.
                if let Some(p) = hub.position(client_id) {
                    let (pcx, pcz) = ((p.x / 32.0).floor() as i32, (p.z / 32.0).floor() as i32);
                    let mut todo: Vec<(i32, i32)> = (pcx - BUILT_RADIUS..=pcx + BUILT_RADIUS)
                        .flat_map(|x| (pcz - BUILT_RADIUS..=pcz + BUILT_RADIUS).map(move |z| (x, z)))
                        .filter(|c| !columns.contains(c))
                        .collect();
                    todo.sort_by_key(|(x, z)| (x - pcx).pow(2) + (z - pcz).pow(2));
                    for (cx, cz) in todo.into_iter().take(STREAM_PER_ROUND) {
                        let overlay = |x, y, z| hub.changes_in(x, y, z);
                        for p in column(cx, cz, &layers, generator.as_ref(), &overlay, &|b| hub.opaque(b))? {
                            let mut wire = Vec::new();
                            frame::encode(p.id, &p.payload, &mut wire)?;
                            streams.chunks.write_all(&wire).await?;
                        }
                        for (id, payload) in hub.damage_packets(cx, cz) {
                            let mut wire = Vec::new();
                            frame::encode(id, &payload, &mut wire)?;
                            streams.chunks.write_all(&wire).await?;
                        }
                        columns.insert((cx, cz));
                        hub.columns_sent(client_id, [(cx, cz)]);
                    }
                }
            }
            _ = report.tick() => {
                seen.flush();
                let pos = hub.position(client_id);
                if pos != last_pos {
                    if let Some(p) = &pos {
                        info!("{} at {:.1} {:.1} {:.1}", player.username, p.x, p.y, p.z);
                    }
                    last_pos = pos;
                }
            }
        }
    }
    seen.flush();
    info!("{} left the world", player.username);
    Ok(())
}

/// Handles one client packet in the world. `false` when the client left.
fn client_packet(f: &Frame, hub: &Hub, client_id: i32, ready: &mut Ready, seen: &mut Seen) -> bool {
    seen.record(f);
    let handled = match f.id {
        world::id::CLIENT_MOVEMENT => Some(hub.movement(client_id, &f.payload)),
        world::id::CHAT_MESSAGE => Some(hub.chat(client_id, &f.payload)),
        world::id::MOVE_ITEM_STACK
        | world::id::SMART_MOVE_ITEM_STACK
        | world::id::INVENTORY_ACTION
        | world::id::DROP_ITEM_STACK
        | world::id::DROP_CREATIVE_ITEM => Some(hub.inventory_action(client_id, f.id, &f.payload)),
        world::id::CLIENT_OPEN_WINDOW => Some(hub.client_open_window(client_id, &f.payload)),
        world::id::SEND_WINDOW_ACTION => Some(hub.window_action(client_id, &f.payload)),
        world::id::CLOSE_WINDOW => Some(hub.close_window(client_id, &f.payload)),
        world::id::SET_CREATIVE_ITEM | world::id::SMART_GIVE_CREATIVE_ITEM => {
            Some(hub.creative_item(client_id, f.id, &f.payload))
        }
        world::id::SYNC_INTERACTION_CHAINS => Some(hub.interactions(client_id, &f.payload)),
        world::id::SYNC_PLAYER_PREFERENCES => Some(hub.player_preferences(client_id, &f.payload)),
        world::id::SET_ACTIVE_SLOT => Some(hub.set_active_slot(client_id, &f.payload).map(|ok| {
            if !ok {
                // The official server disconnects here; Aerie keeps the
                // client and ignores the request.
                warn!("refused SetActiveSlot from {client_id}");
            }
        })),
        _ => None,
    };
    if let Some(r) = handled {
        if let Err(e) = r {
            warn!("cannot read {}: {e:#}", f.id);
        }
        seen.add(f.id);
        return true;
    }
    match f.id {
        id::CLIENT_READY => {
            let flags = f.payload.first().copied().unwrap_or(0);
            if flags & 1 != 0 && !ready.chunks {
                ready.chunks = true;
                info!("← ClientReady: ready for chunks");
            }
            if flags & 2 != 0 && !ready.gameplay {
                ready.gameplay = true;
                hub.player_ready(client_id);
                if let Some(at) = ready.spawn.take()
                    && let Err(e) = hub.teleport(client_id, at)
                {
                    warn!("cannot send the spawn teleport: {e:#}");
                }
                info!("← ClientReady: ready for gameplay");
            }
        }
        id::DISCONNECT => {
            info!("← ClientDisconnect");
            return false;
        }
        id::PONG => hub.pong(client_id, &f.payload),
        other => seen.add(other),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_world_encodes() {
        let w = built(1, &PlayerParts::default(), None, SPAWN, false, 32).unwrap();
        let layers = terrain_layers(|k| Some(k.len() as i32));
        let terrain: Vec<Out> = terrain_area(SPAWN)
            .into_iter()
            .flat_map(|at| {
                terrain_column(
                    at,
                    &layers,
                    None,
                    &|_, _, _| Vec::new(),
                    &|_| true,
                    &|_, _| Vec::new(),
                )
                .unwrap()
            })
            .collect();
        let solid = terrain
            .iter()
            .filter(|p| p.id == id::SET_CHUNK && p.payload.len() > 100)
            .count();
        let side = (2 * BUILT_RADIUS + 1) as usize;
        assert_eq!(solid, side * side * 2, "two solid sections per column");
        for p in std::iter::once(&built_head(&None).unwrap().join)
            .chain(&w.after)
            .chain(&terrain)
        {
            let mut wire = Vec::new();
            frame::encode(p.id, &p.payload, &mut wire).unwrap();
        }
        assert!(terrain.iter().any(|p| p.channel() == Channel::Chunks));
        // The tintmap layout matches the captured sizes (51 colours: 1490
        // bytes) at one colour.
        assert_eq!(uniform_tint(GRASS_TINT).len(), 2 + 4 + 4 + 1280);
    }

    #[test]
    fn capture_is_split_around_join_world() {
        let c = vec![
            (40, vec![1]),
            (22, vec![]),
            (238, vec![2]),
            (3, vec![0; 28]),
            (104, vec![3]),
            (131, vec![4]),
            (100, vec![5]),
            (2, vec![]),
        ];
        let (head, w) = from_capture(&c).unwrap();
        assert_eq!(head.before.iter().map(|p| p.id).collect::<Vec<_>>(), [238]);
        assert_eq!(head.join.payload, [3]);
        assert_eq!(w.after.iter().map(|p| p.id).collect::<Vec<_>>(), [131, 100]);
    }
}
