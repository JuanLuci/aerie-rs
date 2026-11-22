//! `UpdateInteractions` (66) from `Item/Interactions`, indexed (spec 07).
//! Most interactions are contained: written inline in root interactions,
//! items, blocks and other interactions. Inheritance follows the schema; a
//! polymorphic `Type` picks the server type, and `TYPES` says which packet
//! variant it becomes and where it waits for data.
//!
//! Every interaction sends the base fields: `WaitForDataFrom` (by type),
//! `Effects` (always present), `HorizontalSpeedMultiplier` (1), `RunTime`
//! (0), `OnItemChangeBehavior` (Cancel), `Settings` `{game mode:
//! {AllowSkipOnClick}}`, `Rules`, `Tags`, `Camera` `{FirstPerson,
//! ThirdPerson: [{Time (0.1), Position, Rotation}]}`. Links to other
//! interactions (`Next`, `Failed`, …) are interaction indexes, `i32::MIN`
//! when absent; links to root interactions are root indexes.
//!
//! `refs` lists an interaction's links in a fixed order (the order
//! `packet_refs` reads them back from a packet), which lets a compare run
//! carry the official indexes from the root interactions down every chain.

use aerie_protocol::proto::{self as p, InteractionType, WaitForDataFrom as W};
use aerie_protocol::wire::Wire;
use serde_json::Value;

use super::music::{owner_of, reference};
use super::root_interactions::{self, INTERACTIONS as PATH, INTERACTIONS_BUILTIN as BUILTIN};
use super::{entity_effects, entity_stat_types, model_particles, sound_events};
use crate::ctx::{Ctx, enum_or};
use crate::store::Store;
use crate::values::{Get, direction, vec3};
use crate::{Result, tags};

/// Server type → (where it waits for data, packet variant).
fn kind(ty: &str) -> (W, &'static str) {
    match ty {
        "AddItem" => (W::CLIENT, "SimpleBlockInteraction"),
        "ApplyAbilityEffects" => (W::NONE, "SimpleInteraction"),
        "ApplyEffect" => (W::NONE, "ApplyEffectInteraction"),
        "ApplyForce" => (W::CLIENT, "ApplyForceInteraction"),
        "AttachBeam" => (W::NONE, "SimpleInteraction"),
        "AugmentCondition" => (W::SERVER, "SimpleInteraction"),
        "Bed" => (W::CLIENT, "SimpleBlockInteraction"),
        "BlockCondition" => (W::CLIENT, "BlockConditionInteraction"),
        "BreakBlock" => (W::CLIENT, "BreakBlockInteraction"),
        "BuilderTool" => (W::CLIENT, "BuilderToolInteraction"),
        "Camera" => (W::CLIENT, "CameraInteraction"),
        "CameraShake" => (W::NONE, "SimpleInteraction"),
        "CanBreakRespawnPoint" => (W::SERVER, "SimpleBlockInteraction"),
        "CancelChain" => (W::NONE, "CancelChainInteraction"),
        "CarryBlock" => (W::CLIENT, "SimpleBlockInteraction"),
        "CarryDroppedBlock" => (W::NONE, "SimpleInteraction"),
        "CarryPlaceBlock" => (W::CLIENT, "PlaceBlockInteraction"),
        "ChainFlag" => (W::NONE, "ChainFlagInteraction"),
        "Chaining" => (W::CLIENT, "ChainingInteraction"),
        "ChangeActiveSlot" => (W::NONE, "ChangeActiveSlotInteraction"),
        "ChangeBlock" => (W::CLIENT, "ChangeBlockInteraction"),
        "ChangeFarmingStage" => (W::SERVER, "SimpleBlockInteraction"),
        "ChangeStat" => (W::NONE, "ChangeStatInteraction"),
        "ChangeStatWithModifier" => (W::NONE, "SimpleInteraction"),
        "ChangeState" => (W::CLIENT, "ChangeStateInteraction"),
        "Charging" => (W::CLIENT, "ChargingInteraction"),
        "CheckUniqueItemUsage" => (W::SERVER, "SimpleInteraction"),
        "ClearCarriedBlock" => (W::NONE, "SimpleInteraction"),
        "ClearEntityEffect" => (W::NONE, "ClearEntityEffectInteraction"),
        "Condition" => (W::NONE, "ConditionInteraction"),
        "ContextualUseNPC" => (W::NONE, "SimpleInteraction"),
        "CooldownCondition" => (W::NONE, "CooldownConditionInteraction"),
        "CycleBlockGroup" => (W::CLIENT, "SimpleBlockInteraction"),
        "DamageEntity" => (W::NONE, "DamageEntityInteraction"),
        "DestroyBlock" => (W::NONE, "SimpleInteraction"),
        "DestroyTaggedVolumes" => (W::NONE, "SimpleInteraction"),
        "Door" => (W::CLIENT, "SimpleBlockInteraction"),
        "DragEraseBlock" => (W::CLIENT, "DragEraseBlockInteraction"),
        "DragPlaceBlock" => (W::CLIENT, "DragPlaceBlockInteraction"),
        "DurabilityCondition" => (W::NONE, "DurabilityConditionInteraction"),
        "EffectCondition" => (W::NONE, "EffectConditionInteraction"),
        "EquipItem" => (W::SERVER, "SimpleInteraction"),
        "EventStartInteraction" => (W::NONE, "SimpleInteraction"),
        "EventStopInteraction" => (W::NONE, "SimpleInteraction"),
        "ExitInstance" => (W::SERVER, "SimpleInteraction"),
        "Explode" => (W::NONE, "SimpleInteraction"),
        "ExtrudePlaceBlock" => (W::CLIENT, "ExtrudePlaceBlockInteraction"),
        "FertilizeSoil" => (W::SERVER, "SimpleBlockInteraction"),
        "FirstClick" => (W::CLIENT, "FirstClickInteraction"),
        "GameFlagCondition" => (W::SERVER, "SimpleInteraction"),
        "GlobalEventStartInteraction" => (W::NONE, "SimpleInteraction"),
        "GlobalEventStopInteraction" => (W::NONE, "SimpleInteraction"),
        "GrantAbilityCharges" => (W::NONE, "SimpleInteraction"),
        "GroundedCondition" => (W::NONE, "GroundedConditionInteraction"),
        "HarvestCrop" => (W::CLIENT, "SimpleBlockInteraction"),
        "HubPortal" => (W::SERVER, "SimpleInteraction"),
        "IncreaseBackpackCapacity" => (W::SERVER, "SimpleInteraction"),
        "IncrementCooldown" => (W::NONE, "IncrementCooldownInteraction"),
        "Interrupt" => (W::NONE, "SimpleInteraction"),
        "LaunchAbilityProjectile" => (W::NONE, "SimpleInteraction"),
        "LaunchPad" => (W::SERVER, "SimpleBlockInteraction"),
        "LaunchProjectile" => (W::NONE, "SimpleInteraction"),
        "LearnRecipe" => (W::SERVER, "SimpleInteraction"),
        "MarkAbilityBenchIntroSeen" => (W::SERVER, "SimpleInteraction"),
        "MemoriesCondition" => (W::SERVER, "MemoriesConditionInteraction"),
        "ModifyIntervalConditionInteraction" => (W::NONE, "SimpleInteraction"),
        "ModifyInventory" => (W::SERVER, "ModifyInventoryInteraction"),
        "Mount" => (W::NONE, "SimpleInteraction"),
        "MovementCondition" => (W::CLIENT, "MovementConditionInteraction"),
        "OpenAbilityBench" => (W::NONE, "SimpleInteraction"),
        "OpenBenchPage" => (W::CLIENT, "SimpleBlockInteraction"),
        "OpenChapterRewards" => (W::NONE, "SimpleInteraction"),
        "OpenContainer" => (W::CLIENT, "SimpleBlockInteraction"),
        "OpenCustomUI" => (W::NONE, "SimpleInteraction"),
        "OpenItemStackContainer" => (W::NONE, "SimpleInteraction"),
        "OpenPage" => (W::NONE, "SimpleInteraction"),
        "OpenProcessingBench" => (W::CLIENT, "SimpleBlockInteraction"),
        "Parallel" => (W::NONE, "ParallelInteraction"),
        "PickBlock" => (W::CLIENT, "PickBlockInteraction"),
        "PickupItem" => (W::NONE, "SimpleInteraction"),
        "PlaceBlock" => (W::CLIENT, "PlaceBlockInteraction"),
        "PlaceFluid" => (W::CLIENT, "SimpleBlockInteraction"),
        "PlaceModeSelect" => (W::NONE, "PlaceModeSelectInteraction"),
        "PlacementCountCondition" => (W::SERVER, "SimpleInteraction"),
        "Portal" => (W::SERVER, "SimpleBlockInteraction"),
        "PortalReturn" => (W::SERVER, "SimpleBlockInteraction"),
        "PrefabSelectionInteraction" => (W::NONE, "SimpleInteraction"),
        "Projectile" => (W::CLIENT, "ProjectileInteraction"),
        "PullEntityAbility" => (W::NONE, "SimpleInteraction"),
        "RefillContainer" => (W::NONE, "SimpleInteraction"),
        "RemoveEntity" => (W::NONE, "RemoveEntityInteraction"),
        "Repeat" => (W::NONE, "RepeatInteraction"),
        "Replace" => (W::NONE, "ReplaceInteraction"),
        "ResetCooldown" => (W::NONE, "ResetCooldownInteraction"),
        "RevealMapMarkersInView" => (W::SERVER, "SimpleInteraction"),
        "RunOnBlockTypes" => (W::SERVER, "SimpleInteraction"),
        "RunRootInteraction" => (W::NONE, "RunRootInteraction"),
        "Seating" => (W::CLIENT, "SimpleBlockInteraction"),
        "Selector" => (W::CLIENT, "SelectInteraction"),
        "SendBeacon" => (W::NONE, "SimpleInteraction"),
        "SendMessage" => (W::NONE, "SimpleInteraction"),
        "Serial" => (W::NONE, "SerialInteraction"),
        "SetGameFlag" => (W::SERVER, "SimpleInteraction"),
        "SetMemoriesCapacity" => (W::SERVER, "SimpleInteraction"),
        "ShowEventTitle" => (W::NONE, "SimpleInteraction"),
        "SignalNearbyVolumes" => (W::NONE, "SimpleInteraction"),
        "Simple" => (W::NONE, "SimpleInteraction"),
        "SpawnAbilityEntity" => (W::NONE, "SimpleInteraction"),
        "SpawnDeployableAtHitLocation" => (W::NONE, "SimpleInteraction"),
        "SpawnDeployableAtLocation" => (W::NONE, "SimpleInteraction"),
        "SpawnDeployableFromRaycast" => (W::CLIENT, "SpawnDeployableFromRaycastInteraction"),
        "SpawnMinecart" => (W::CLIENT, "SimpleBlockInteraction"),
        "SpawnNPC" => (W::CLIENT, "SimpleBlockInteraction"),
        "SpawnPrefab" => (W::NONE, "SimpleInteraction"),
        "SpawnTriggerVolume" => (W::NONE, "SimpleInteraction"),
        "SpectateControl" => (W::NONE, "SimpleInteraction"),
        "SplitAbilityProjectile" => (W::NONE, "SimpleInteraction"),
        "StartObjective" => (W::NONE, "SimpleInteraction"),
        "StatsCondition" => (W::NONE, "StatsConditionInteraction"),
        "StatsConditionWithModifier" => (W::NONE, "SimpleInteraction"),
        "SurfaceDrawPlaceBlock" => (W::CLIENT, "SurfaceDrawPlaceBlockInteraction"),
        "TeleportConfigInstance" => (W::SERVER, "SimpleBlockInteraction"),
        "TeleportInstance" => (W::SERVER, "SimpleInteraction"),
        "Teleporter" => (W::SERVER, "SimpleBlockInteraction"),
        "ToggleGlider" => (W::NONE, "ToggleGliderInteraction"),
        "TriggerAbilityFX" => (W::NONE, "SimpleInteraction"),
        "TriggerCooldown" => (W::NONE, "TriggerCooldownInteraction"),
        "TriggerSpawnMarkers" => (W::NONE, "SimpleInteraction"),
        "UseBlock" => (W::CLIENT, "UseBlockInteraction"),
        "UseCaptureCrate" => (W::CLIENT, "SimpleBlockInteraction"),
        "UseCoop" => (W::CLIENT, "SimpleBlockInteraction"),
        "UseEntity" => (W::CLIENT, "UseEntityInteraction"),
        "UseNPC" => (W::NONE, "SimpleInteraction"),
        "UseWateringCan" => (W::CLIENT, "SimpleBlockInteraction"),
        "Wielding" => (W::CLIENT, "WieldingInteraction"),
        _ => (W::NONE, "SimpleInteraction"),
    }
}

/// Resolves links while building one interaction, recording the linked
/// interaction keys in order.
pub struct Refs<'a> {
    ctx: &'a Ctx,
    store: &'a Store,
    key: &'a str,
    pub keys: Vec<Option<String>>,
}

impl Refs<'_> {
    /// The key a link field names: a string, or the generated key of an
    /// inline interaction (`path` below the owner of `field`).
    fn link_key(&self, v: Option<&Value>, field: &str, path: &str) -> Option<String> {
        let v = v?;
        let owner = owner_of(self.store, self.key, field);
        reference(v, &owner, path)
    }

    /// An interaction link (recorded).
    fn i(&mut self, a: &Value, field: &str) -> Result<i32> {
        let k = self.link_key(a.get(field), field, field);
        self.i_key(k)
    }

    fn i_key(&mut self, k: Option<String>) -> Result<i32> {
        let i = match &k {
            Some(k) => self.ctx.index_or(PATH, BUILTIN, Some(k), i32::MIN)?,
            None => i32::MIN,
        };
        self.keys.push(k);
        Ok(i)
    }

    /// A root interaction link (not recorded).
    fn root(&self, a: &Value, field: &str) -> Result<i32> {
        let k = self.link_key(a.get(field), field, field);
        self.root_key(k.as_deref())
    }

    fn root_key(&self, k: Option<&str>) -> Result<i32> {
        match k {
            Some(k) => self.ctx.index_or(
                root_interactions::PATH,
                root_interactions::BUILTIN,
                Some(k),
                i32::MIN,
            ),
            None => Ok(i32::MIN),
        }
    }
}

fn vec3_or(v: &Value, key: &str) -> [f32; 3] {
    v.obj(key).map(vec3).unwrap_or([0.0, 0.0, 0.0])
}

fn effects(
    ctx: &Ctx,
    store: &Store,
    key: &str,
    v: Option<&Value>,
) -> Result<p::InteractionEffects> {
    let null = Value::Null;
    let v = v.unwrap_or(&null);
    let world = sound_events::index(ctx, v.str_at("WorldSoundEventId").as_deref())?;
    let local = sound_events::index(ctx, v.str_at("LocalSoundEventId").as_deref())?;
    let parts = |k: &str| {
        v.get(k)
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .map(|a| a.iter().map(model_particles::one).collect())
    };
    let owner = owner_of(store, key, "Effects");
    // Zoom: unset values resolve from the others.
    let zoom = v.obj("Zoom").map(|z| {
        let mag = Some(z.f32_or("MagnificationMultiplier", 4.0))
            .filter(|m| *m > 0.0)
            .unwrap_or(4.0);
        let lod = Some(z.f32_or("LodMultiplier", 0.0))
            .filter(|l| *l > 0.0)
            .unwrap_or(mag);
        let lerp = z.f32_or("TransitionLerp", 0.4);
        let fade = z.f32_or("OverlayFadePerSecond", 3.0).max(0.0);
        let or_nan = |k: &str, d: f32| z.get(k).and_then(Value::as_f64).map_or(d, |x| x as f32);
        p::ZoomConfig {
            magnification_multiplier: mag,
            mouse_sensitivity_multiplier: z.f32_or("MouseSensitivityMultiplier", 0.5),
            overlay_texture: z.str_at("OverlayTexture"),
            depth_of_field: z.bool_or("DepthOfField", false),
            allow_camera_orbit: z.bool_or("AllowCameraOrbit", false),
            force_perspective: enum_or(
                z,
                "ForcePerspective",
                p::ForcedPerspective::from_name,
                Default::default(),
            ),
            lod_multiplier: lod,
            in_lerp: or_nan("InLerp", lerp),
            out_lerp: or_nan("OutLerp", lerp),
            overlay_fade: z.bool_or("OverlayFade", true),
            overlay_fade_in_per_second: or_nan("OverlayFadeInPerSecond", fade),
            overlay_fade_out_per_second: or_nan("OverlayFadeOutPerSecond", fade),
        }
    });
    Ok(p::InteractionEffects {
        particles: parts("Particles"),
        first_person_particles: parts("FirstPersonParticles"),
        world_sound_event_index: world,
        local_sound_event_index: if local != 0 { local } else { world },
        trails: v.get("Trails").and_then(Value::as_array).map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, t)| p::ModelTrail {
                    trail_id: t
                        .get("TrailId")
                        .and_then(|x| reference(x, &owner, &format!("Effects_Trails_{i}_TrailId"))),
                    target_entity_part: enum_or(
                        t,
                        "TargetEntityPart",
                        p::EntityPart::from_name,
                        p::EntityPart::SELF,
                    ),
                    target_node_name: t.str_at("TargetNodeName"),
                    position_offset: t.obj("PositionOffset").map(vec3),
                    rotation_offset: t.obj("RotationOffset").map(direction),
                    fixed_rotation: t.bool_or("FixedRotation", false),
                })
                .collect()
        }),
        wait_for_animation_to_finish: v.bool_or("WaitForAnimationToFinish", false),
        item_player_animations_id: v
            .get("ItemPlayerAnimationsId")
            .and_then(|x| reference(x, &owner, "Effects_ItemPlayerAnimationsId")),
        item_animation_id: v.str_at("ItemAnimationId"),
        clear_animation_on_finish: v.bool_or("ClearAnimationOnFinish", false),
        clear_sound_event_on_finish: v.bool_or("ClearSoundEventOnFinish", false),
        hide_first_person_held_item: v.bool_or("HideFirstPersonHeldItem", false),
        camera_shake: match v
            .get("CameraEffect")
            .and_then(|x| reference(x, &owner, "Effects_CameraEffect"))
        {
            Some(k) => camera_shake(ctx, &k)?,
            None => None,
        },
        movement_effects: v.obj("MovementEffects").map(entity_effects::movement),
        persist_zoom: zoom.is_some() && v.bool_or("PersistZoom", false),
        zoom,
        start_delay: v.f32_or("StartDelay", 0.0),
    })
}

/// A camera effect's shake (`Camera/CameraEffect`, type `CameraShake`):
/// the camera shake index, the intensity (`Intensity.Value` passed through
/// `Intensity.Modifier`, a piecewise linear `Input` → `Output` map) and the
/// accumulation mode (Set).
fn camera_shake(ctx: &Ctx, key: &str) -> Result<Option<p::CameraShakeEffect>> {
    let effects = ctx.store("Camera/CameraEffect")?;
    let Some(k) = effects
        .own
        .keys()
        .find(|k| k.eq_ignore_ascii_case(key))
        .cloned()
    else {
        return Ok(None);
    };
    let e = effects.resolved(&k, "CameraEffect");
    if e.str_at("Type")
        .as_deref()
        .is_some_and(|t| t != "CameraShake")
    {
        return Ok(None);
    }
    let shake = e
        .get("CameraShake")
        .and_then(|x| reference(x, &owner_of(&effects, &k, "CameraShake"), "CameraShake"));
    let camera_shake_id =
        ctx.index_or(super::camera_shake::PATH, &[], shake.as_deref(), i32::MIN)?;
    let i = e.obj("Intensity");
    let value = i.map_or(0.0, |i| i.f32_or("Value", 0.0));
    let floats = |m: &Value, k: &str| -> Vec<f32> {
        m.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_f64)
                    .map(|x| x as f32)
                    .collect()
            })
            .unwrap_or_default()
    };
    let intensity = match i.and_then(|i| i.obj("Modifier")) {
        Some(m) => {
            let (input, output) = (floats(m, "Input"), floats(m, "Output"));
            if input.is_empty() || output.is_empty() {
                value
            } else if value < input[0] {
                output[0]
            } else {
                let (mut lo_i, mut lo_o) = (input[0], output[0]);
                let mut out = None;
                for j in 1..input.len().min(output.len()) {
                    let (hi_i, hi_o) = (input[j], output[j]);
                    if value <= hi_i {
                        out = Some(lo_o + (value - lo_i) * (hi_o - lo_o) / (hi_i - lo_i));
                        break;
                    }
                    lo_i = hi_i;
                    lo_o = hi_o;
                }
                out.unwrap_or(lo_o)
            }
        }
        None => value,
    };
    Ok(Some(p::CameraShakeEffect {
        camera_shake_id,
        intensity,
        mode: i
            .and_then(|i| i.str_at("AccumulationMode"))
            .as_deref()
            .and_then(p::AccumulationMode::from_name)
            .unwrap_or_default(),
    }))
}

fn camera(v: &Value) -> p::InteractionCameraSettings {
    let list = |k: &str| {
        v.get(k).and_then(Value::as_array).map(|a| {
            a.iter()
                .map(|c| p::InteractionCamera {
                    time: c.f32_or("Time", 0.1),
                    position: vec3_or(c, "Position"),
                    rotation: Some(c.obj("Rotation").map(direction).unwrap_or_default()),
                })
                .collect()
        })
    };
    p::InteractionCameraSettings {
        first_person: list("FirstPerson"),
        third_person: list("ThirdPerson"),
    }
}

/// The base fields, shared by every variant.
struct Base {
    wait: W,
    effects: p::InteractionEffects,
    hsm: f32,
    run_time: f32,
    on_item_change: p::InteractionItemChangeBehavior,
    settings: Option<Vec<(p::GameMode, p::InteractionSettings)>>,
    rules: p::InteractionRules,
    tags: Option<Vec<i32>>,
    camera: Option<p::InteractionCameraSettings>,
}

macro_rules! iv {
    ($v:ident, $b:expr, { $($f:tt)* }) => {
        p::Interaction::$v(Box::new(p::$v {
            wait_for_data_from: $b.wait,
            effects: Some($b.effects.clone()),
            horizontal_speed_multiplier: $b.hsm,
            run_time: $b.run_time,
            on_item_change_behavior: $b.on_item_change,
            settings: $b.settings.clone(),
            rules: $b.rules.clone(),
            tags: $b.tags.clone(),
            camera: $b.camera.clone(),
            $($f)*
        }))
    };
}

fn target(v: &Value, key: &str) -> p::InteractionTarget {
    enum_or(
        v,
        key,
        p::InteractionTarget::from_name,
        p::InteractionTarget::USER,
    )
}

fn stat_map(ctx: &Ctx, v: Option<&Value>) -> Result<Option<Vec<(i32, f32)>>> {
    let Some(m) = v.and_then(Value::as_object).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    let mut out = Vec::new();
    for (stat, amount) in m {
        let i = ctx.index_or(
            entity_stat_types::PATH,
            entity_stat_types::BUILTIN,
            Some(stat),
            i32::MIN,
        )?;
        if i != i32::MIN {
            out.push((i, amount.as_f64().unwrap_or(0.0) as f32));
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

fn damage_effects(ctx: &Ctx, v: &Value) -> Result<p::DamageEffects> {
    let parts = v
        .get("ModelParticles")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| a.iter().map(model_particles::one).collect());
    let world_parts = v
        .get("WorldParticles")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.iter()
                .map(|w| p::WorldParticle {
                    system_id: w.str_at("SystemId"),
                    scale: w.f32_or("Scale", 1.0),
                    color: w.color("Color"),
                    position_offset: w.obj("PositionOffset").map(vec3),
                    rotation_offset: w.obj("RotationOffset").map(direction),
                })
                .collect()
        });
    let world = sound_events::index(ctx, v.str_at("WorldSoundEventId").as_deref())?;
    let local = sound_events::index(ctx, v.str_at("LocalSoundEventId").as_deref())?;
    Ok(p::DamageEffects {
        model_particles: parts,
        world_particles: world_parts,
        sound_event_index: if local != 0 { local } else { world },
    })
}

fn cooldown(v: Option<&Value>) -> Option<p::InteractionCooldown> {
    v.filter(|v| v.is_object()).map(root_interactions::cooldown)
}

fn deg(v: &Value, key: &str, def: f64) -> f32 {
    v.get(key)
        .and_then(Value::as_f64)
        .unwrap_or(def)
        .to_radians() as f32
}

fn selector(v: Option<&Value>) -> p::Selector {
    let null = Value::Null;
    let s = v.unwrap_or(&null);
    let offset = Some(vec3_or(s, "Offset"));
    let anchor = enum_or(
        s,
        "Anchor",
        p::SelectorAnchor::from_name,
        Default::default(),
    );
    match s.str_at("Id").as_deref() {
        Some("AOECylinder") => p::Selector::AOECylinderSelector(Box::new(p::AOECylinderSelector {
            range: s.f32_or("Range", 0.0),
            height: s.f32_or("Height", 0.0),
            offset,
        })),
        Some("Raycast") => p::Selector::RaycastSelector(Box::new(p::RaycastSelector {
            offset,
            distance: s.i32_or("Distance", 30),
            block_tag_index: i32::MIN,
            ignore_fluids: s.bool_or("IgnoreFluids", false),
            ignore_empty_collision_material: s.bool_or("IgnoreEmptyCollisionMaterial", false),
            distance_attribute: s.str_at("DistanceAttribute"),
        })),
        Some("Horizontal") => p::Selector::HorizontalSelector(Box::new(p::HorizontalSelector {
            extend_top: s.f32_or("ExtendTop", 1.0),
            extend_bottom: s.f32_or("ExtendBottom", 1.0),
            yaw_length: deg(s, "Length", 0.0),
            yaw_start_offset: deg(s, "YawStartOffset", 0.0),
            pitch_offset: deg(s, "PitchOffset", 0.0),
            roll_offset: deg(s, "RollOffset", 0.0),
            start_distance: s.f32_or("StartDistance", 0.01),
            end_distance: s.f32_or("EndDistance", 0.0),
            direction: match s.str_at("Direction").as_deref() {
                Some("ToLeft") => {
                    p::HorizontalSelectorDirection::from_name("ToLeft").unwrap_or_default()
                }
                _ => p::HorizontalSelectorDirection::from_name("ToRight").unwrap_or_default(),
            },
            test_line_of_sight: s.bool_or("TestLineOfSight", true),
            ignore_pitch: s.bool_or("IgnorePitch", false),
            ignore_yaw: s.bool_or("IgnoreYaw", false),
            anchor,
        })),
        Some("Stab") => p::Selector::StabSelector(Box::new(p::StabSelector {
            extend_top: s.f32_or("ExtendTop", 1.0),
            extend_bottom: s.f32_or("ExtendBottom", 1.0),
            extend_left: s.f32_or("ExtendLeft", 1.0),
            extend_right: s.f32_or("ExtendRight", 1.0),
            yaw_offset: deg(s, "YawOffset", 0.0),
            pitch_offset: deg(s, "PitchOffset", 0.0),
            roll_offset: deg(s, "RollOffset", 0.0),
            start_distance: s.f32_or("StartDistance", 0.01),
            end_distance: s.f32_or("EndDistance", 0.0),
            test_line_of_sight: s.bool_or("TestLineOfSight", false),
            ignore_pitch: s.bool_or("IgnorePitch", false),
            ignore_yaw: s.bool_or("IgnoreYaw", false),
            anchor,
            size_attribute: s.str_at("SizeAttribute"),
        })),
        Some("Donut") => p::Selector::DonutSelector(Box::new(p::DonutSelector {
            min_radius: s.f32_or("MinRadius", 0.0),
            max_radius: s.f32_or("MaxRadius", 0.0),
            width: s.f32_or("Width", 0.1),
            height: s.f32_or("Height", 0.1),
            angle: s
                .get("AngleDeg")
                .and_then(Value::as_f64)
                .map_or(std::f64::consts::PI * 2.0, f64::to_radians) as f32,
            offset,
            yaw_offset: deg(s, "YawOffsetDeg", 0.0),
        })),
        _ => p::Selector::AOECircleSelector(Box::new(p::AOECircleSelector {
            range: s.f32_or("Range", 0.0),
            offset,
            size_attribute: s.str_at("SizeAttribute"),
        })),
    }
}

/// The server type of an interaction: its own `Type`, else its nearest
/// ancestor's.
pub(crate) fn type_of(store: &Store, key: &str) -> String {
    let mut k = key.to_owned();
    for _ in 0..32 {
        if let Some(t) = store.own.get(&k).and_then(|a| a.str_at("Type")) {
            return t;
        }
        match store.parents.get(&k) {
            Some(p) => k = p.clone(),
            None => break,
        }
    }
    "Simple".into()
}

/// Builds one interaction, recording its links in `r.keys`.
pub fn interaction(r: &mut Refs, a: &Value, ty: &str) -> Result<p::Interaction> {
    let (ctx, store, key) = (r.ctx, r.store, r.key);
    let (wait, variant) = kind(ty);
    let settings = a
        .obj("Settings")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(mode, s)| {
                    Some((
                        p::GameMode::from_name(mode)?,
                        p::InteractionSettings {
                            allow_skip_on_click: s.bool_or("AllowSkipOnClick", false),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let b = Base {
        wait,
        effects: effects(ctx, store, key, a.obj("Effects"))?,
        hsm: a.f32_or("HorizontalSpeedMultiplier", 1.0),
        run_time: a.f32_or("RunTime", 0.0),
        // Changing the active slot always ignores item changes.
        on_item_change: if ty == "ChangeActiveSlot" {
            p::InteractionItemChangeBehavior::IGNORE
        } else {
            enum_or(
                a,
                "OnItemChangeBehavior",
                p::InteractionItemChangeBehavior::from_name,
                p::InteractionItemChangeBehavior::CANCEL,
            )
        },
        settings: Some(settings),
        rules: root_interactions::rules(ctx, a.obj("Rules")),
        tags: Some(tags::indexes(ctx, store, key)),
        camera: a.obj("Camera").map(camera),
    };
    let use_latest = a.bool_or("UseLatestTarget", false);
    Ok(match variant {
        "SimpleBlockInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(SimpleBlockInteraction, b, { next, failed, use_latest_target: use_latest })
        }
        "PlaceBlockInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let carry = ty == "CarryPlaceBlock";
            iv!(PlaceBlockInteraction, b, {
                next, failed,
                block_id: if carry { -1 } else {
                    match a.str_at("BlockTypeToPlace") {
                        Some(k) => ctx.index_or(super::blocks::PATH, super::blocks::BUILTIN, Some(&k), i32::MIN)?,
                        None => -1,
                    }
                },
                remove_item_in_hand: !carry && a.bool_or("RemoveItemInHand", true),
                quick_replace: a.bool_or("QuickReplace", false),
                quick_retype: a.bool_or("QuickRetype", false),
                no_physics: a.bool_or("NoPhysics", false),
            })
        }
        "BreakBlockInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(BreakBlockInteraction, b, {
                next, failed, use_latest_target: use_latest,
                harvest: a.bool_or("Harvest", false),
                soft_only: a.bool_or("SoftOnly", false),
            })
        }
        "PickBlockInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(PickBlockInteraction, b, { next, failed, use_latest_target: use_latest })
        }
        "UseBlockInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(UseBlockInteraction, b, { next, failed, use_latest_target: use_latest })
        }
        "UseEntityInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(UseEntityInteraction, b, { next, failed })
        }
        "BuilderToolInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(BuilderToolInteraction, b, { next, failed })
        }
        "ToggleGliderInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(ToggleGliderInteraction, b, { next, failed })
        }
        "ConditionInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let ob = |k: &str| a.get(k).and_then(Value::as_bool);
            iv!(ConditionInteraction, b, {
                next, failed,
                required_game_mode: a.str_at("RequiredGameMode").as_deref().and_then(p::GameMode::from_name),
                jumping: ob("Jumping"),
                swimming: ob("Swimming"),
                crouching: ob("Crouching"),
                running: ob("Running"),
                flying: ob("Flying"),
                gliding: ob("Gliding"),
            })
        }
        "StatsConditionInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(StatsConditionInteraction, b, {
                next, failed,
                costs: stat_map(ctx, a.get("Costs"))?,
                less_than: a.bool_or("LessThan", false),
                lenient: a.bool_or("Lenient", false),
                value_type: enum_or(a, "ValueType", p::ValueType::from_name, p::ValueType::ABSOLUTE),
                use_ability_cost: a.bool_or("UseAbilityCost", false),
            })
        }
        "ChangeStatInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(ChangeStatInteraction, b, {
                next, failed,
                entity_target: target(a, "Entity"),
                value_type: enum_or(a, "ValueType", p::ValueType::from_name, p::ValueType::ABSOLUTE),
                stat_modifiers: stat_map(ctx, a.get("StatModifiers"))?,
                change_stat_behaviour: enum_or(a, "Behaviour", p::ChangeStatBehaviour::from_name, p::ChangeStatBehaviour::from_name("Add").unwrap_or_default()),
                use_ability_cost: a.bool_or("UseAbilityCost", false),
            })
        }
        "ApplyEffectInteraction" | "ClearEntityEffectInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let field = if variant == "ApplyEffectInteraction" {
                "EffectId"
            } else {
                "EntityEffectId"
            };
            let ek = r.link_key(a.get(field), field, field);
            let effect_id = ctx.index_or(entity_effects::PATH, &[], ek.as_deref(), i32::MIN)?;
            let entity_target = target(a, "Entity");
            if variant == "ApplyEffectInteraction" {
                iv!(ApplyEffectInteraction, b, { next, failed, effect_id, entity_target })
            } else {
                iv!(ClearEntityEffectInteraction, b, { next, failed, effect_id, entity_target })
            }
        }
        "RemoveEntityInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(RemoveEntityInteraction, b, { next, failed, entity_target: target(a, "Entity") })
        }
        "EffectConditionInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let ids = a.strings("EntityEffectIds").unwrap_or_default();
            let entity_effects = ids
                .iter()
                .map(|e| ctx.index_or(entity_effects::PATH, &[], Some(e), i32::MIN))
                .collect::<Result<Vec<_>>>()?;
            iv!(EffectConditionInteraction, b, {
                next, failed, entity_effects,
                r#match: enum_or(a, "Match", p::Match::from_name, p::Match::from_name("All").unwrap_or_default()),
                entity_target: target(a, "Entity"),
            })
        }
        "ResetCooldownInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(ResetCooldownInteraction, b, { next, failed, cooldown: cooldown(a.get("Cooldown")) })
        }
        "TriggerCooldownInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(TriggerCooldownInteraction, b, {
                next, failed,
                cooldown: cooldown(a.get("Cooldown")),
                use_ability_cooldown: a.bool_or("UseAbilityCooldown", false),
            })
        }
        "CooldownConditionInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(CooldownConditionInteraction, b, { next, failed, cooldown_id: a.str_at("Id").unwrap_or_default() })
        }
        "ChainFlagInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(ChainFlagInteraction, b, {
                next, failed,
                chain_id: a.str_at("ChainId").unwrap_or_default(),
                flag: a.str_at("Flag").unwrap_or_default(),
            })
        }
        "CancelChainInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(CancelChainInteraction, b, { next, failed, chain_id: a.str_at("ChainId").unwrap_or_default() })
        }
        "IncrementCooldownInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(IncrementCooldownInteraction, b, {
                next, failed,
                cooldown_id: a.str_at("Id"),
                cooldown_increment_time: a.f32_or("Time", 0.0),
                cooldown_increment_charge: a.i32_or("Charge", 0),
                cooldown_increment_charge_time: a.f32_or("ChargeTime", 0.0),
                cooldown_increment_interrupt: a.bool_or("InterruptRecharge", false),
            })
        }
        "RunRootInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let root_interaction = r.root_key(a.str_at("RootInteraction").as_deref())?;
            iv!(RunRootInteraction, b, { next, failed, root_interaction })
        }
        "CameraInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(CameraInteraction, b, {
                next, failed,
                camera_action: enum_or(a, "Action", p::CameraActionType::from_name, Default::default()),
                camera_perspective: enum_or(a, "Perspective", p::CameraPerspectiveType::from_name, Default::default()),
                camera_persist: a.bool_or("PersistCameraState", false),
                camera_interaction_time: a.f32_or("CameraInteractionTime", 0.0),
            })
        }
        "DurabilityConditionInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(DurabilityConditionInteraction, b, {
                next, failed,
                threshold: a.f32_or("Threshold", 0.0),
                value_type: enum_or(a, "ValueType", p::ValueType::from_name, p::ValueType::ABSOLUTE),
                operator: enum_or(a, "Operator", p::ComparisonOperator::from_name, p::ComparisonOperator::from_name("LessOrEqual").unwrap_or_default()),
            })
        }
        "GroundedConditionInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(GroundedConditionInteraction, b, { next, failed, tolerance: a.f32_or("Tolerance", 0.0) })
        }
        "ProjectileInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(ProjectileInteraction, b, {
                next, failed,
                config_id: a.str_at("Config"),
                ignore_pitch: a.bool_or("IgnorePitch", false),
                ignore_yaw: a.bool_or("IgnoreYaw", false),
                ignore_roll: a.bool_or("IgnoreRoll", false),
            })
        }
        "RepeatInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let fork_interactions = r.root(a, "ForkInteractions")?;
            iv!(RepeatInteraction, b, { next, failed, fork_interactions, repeat: a.i32_or("Repeat", 1) })
        }
        "DragPlaceBlockInteraction"
        | "ExtrudePlaceBlockInteraction"
        | "SurfaceDrawPlaceBlockInteraction"
        | "DragEraseBlockInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let fork_interactions = r.root(a, "ForkInteractions")?;
            let (tick, gesture) = (
                a.i32_or("MaxBlocksPerTick", 4),
                a.i32_or("MaxBlocksPerGesture", 512),
            );
            match variant {
                "DragPlaceBlockInteraction" => {
                    iv!(DragPlaceBlockInteraction, b, { next, failed, fork_interactions, max_blocks_per_tick: tick, max_blocks_per_gesture: gesture })
                }
                "ExtrudePlaceBlockInteraction" => {
                    iv!(ExtrudePlaceBlockInteraction, b, { next, failed, fork_interactions, max_blocks_per_tick: tick, max_blocks_per_gesture: gesture })
                }
                "SurfaceDrawPlaceBlockInteraction" => {
                    iv!(SurfaceDrawPlaceBlockInteraction, b, { next, failed, fork_interactions, max_blocks_per_tick: tick, max_blocks_per_gesture: gesture })
                }
                _ => {
                    iv!(DragEraseBlockInteraction, b, { next, failed, fork_interactions, max_blocks_per_tick: tick, max_blocks_per_gesture: gesture })
                }
            }
        }
        "PlaceModeSelectInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let d = r.i(a, "DefaultInteractions")?;
            let rp = r.i(a, "ReplaceInteractions")?;
            let rt = r.i(a, "RetypeInteractions")?;
            let ex = r.i(a, "ExtrudeInteractions")?;
            let sd = r.i(a, "SurfaceDrawInteractions")?;
            let fp = r.i(a, "FastPlaceInteractions")?;
            iv!(PlaceModeSelectInteraction, b, {
                next, failed,
                default_interactions: d, replace_interactions: rp, retype_interactions: rt,
                extrude_interactions: ex, surface_draw_interactions: sd, fast_place_interactions: fp,
            })
        }
        "MovementConditionInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let mut m = Vec::new();
            for k in [
                "Forward",
                "Back",
                "Left",
                "Right",
                "ForwardLeft",
                "ForwardRight",
                "BackLeft",
                "BackRight",
            ] {
                m.push(r.i(a, k)?);
            }
            iv!(MovementConditionInteraction, b, {
                next, failed,
                forward: m[0], back: m[1], left: m[2], right: m[3],
                forward_left: m[4], forward_right: m[5], back_left: m[6], back_right: m[7],
            })
        }
        "FirstClickInteraction" => {
            let (click, held) = (r.i(a, "Click")?, r.i(a, "Held")?);
            iv!(FirstClickInteraction, b, { click, held })
        }
        "SerialInteraction" => {
            let list = a
                .get("Interactions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut serial_interactions = Vec::new();
            for (i, x) in list.iter().enumerate() {
                let k = r.link_key(Some(x), "Interactions", &format!("Interactions_{i}"));
                serial_interactions.push(r.i_key(k)?);
            }
            iv!(SerialInteraction, b, { serial_interactions })
        }
        "ParallelInteraction" => {
            let list = a
                .get("Interactions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut next = Vec::new();
            for (i, x) in list.iter().enumerate() {
                let k = r.link_key(Some(x), "Interactions", &format!("Interactions_{i}"));
                next.push(r.root_key(k.as_deref())?);
            }
            iv!(ParallelInteraction, b, { next })
        }
        "ReplaceInteraction" => {
            let default_value = r.root(a, "DefaultValue")?;
            iv!(ReplaceInteraction, b, { default_value, variable: a.str_at("Var") })
        }
        "ChangeActiveSlotInteraction" => {
            iv!(ChangeActiveSlotInteraction, b, { target_slot: a.i32_or("TargetSlot", i32::MIN) })
        }
        "ChainingInteraction" => {
            let list = a
                .get("Next")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut chaining_next = Vec::new();
            for (i, x) in list.iter().enumerate() {
                let k = r.link_key(Some(x), "Next", &format!("Next_{i}"));
                chaining_next.push(r.i_key(k)?);
            }
            let flags = match a.obj("Flags").and_then(Value::as_object) {
                Some(m) => {
                    let mut keys: Vec<&String> = m.keys().collect();
                    keys.sort();
                    let mut out = Vec::new();
                    for f in keys {
                        let k = r.link_key(m.get(f), "Flags", &format!("Flags_{f}"));
                        out.push((f.clone(), r.i_key(k)?));
                    }
                    Some(out)
                }
                None => None,
            };
            iv!(ChainingInteraction, b, {
                chain_id: a.str_at("ChainId"),
                chaining_allowance: a.f32_or("ChainingAllowance", 0.0),
                chaining_next,
                flags,
            })
        }
        "ChargingInteraction" | "WieldingInteraction" => {
            let failed = r.i(a, "Failed")?;
            let charged_next = match a.get("Next") {
                // Wielding: one interaction, sent at time 0.
                Some(v) if variant == "WieldingInteraction" => {
                    let k = r.link_key(Some(v), "Next", "Next");
                    Some(vec![(0.0, r.i_key(k)?)])
                }
                Some(Value::Object(m)) => {
                    let mut keys: Vec<(f32, &String)> = m
                        .keys()
                        .map(|k| (k.parse::<f32>().unwrap_or(0.0), k))
                        .collect();
                    keys.sort_by(|x, y| x.0.total_cmp(&y.0));
                    let mut out = Vec::new();
                    for (t, s) in keys {
                        let k = r.link_key(m.get(s), "Next", &format!("Next_{s}"));
                        out.push((t, r.i_key(k)?));
                    }
                    Some(out)
                }
                Some(v @ Value::String(_)) => {
                    let k = r.link_key(Some(v), "Next", "Next");
                    Some(vec![(0.0, r.i_key(k)?)])
                }
                _ => None,
            };
            let forks = match a.obj("Forks").and_then(Value::as_object) {
                Some(m) => {
                    let mut out = Vec::new();
                    for (t, x) in m {
                        let Some(ty) = InteractionType::from_name(t) else {
                            continue;
                        };
                        let k = r.link_key(Some(x), "Forks", &format!("Forks_{t}"));
                        out.push((ty, r.root_key(k.as_deref())?));
                    }
                    Some(out)
                }
                None => None,
            };
            let charging_delay = a.obj("Delay").map(|d| p::ChargingDelay {
                min_delay: d.f32_or("MinDelay", 0.0),
                max_delay: d.f32_or("MaxDelay", 0.0),
                max_total_delay: d.f32_or("MaxTotalDelay", 0.0),
                min_health: d.f32_or("MinHealth", 0.0),
                max_health: d.f32_or("MaxHealth", 0.0),
            });
            let (allow_indefinite_hold, display_progress, cancel_on_other_click, fail_on_damage) = (
                a.bool_or("AllowIndefiniteHold", variant == "WieldingInteraction"),
                a.bool_or("DisplayProgress", true),
                a.bool_or("CancelOnOtherClick", true),
                a.bool_or("FailOnDamage", false),
            );
            let (mst, msd) = (
                a.f32_or("MouseSensitivityAdjustmentTarget", 1.0),
                a.f32_or("MouseSensitivityAdjustmentDuration", 1.0),
            );
            if variant == "ChargingInteraction" {
                iv!(ChargingInteraction, b, {
                    failed, allow_indefinite_hold, display_progress, cancel_on_other_click, fail_on_damage,
                    mouse_sensitivity_adjustment_target: mst, mouse_sensitivity_adjustment_duration: msd,
                    charged_next, forks, charging_delay,
                })
            } else {
                let has =
                    a.get("DamageModifiers").is_some() || a.get("KnockbackModifiers").is_some();
                iv!(WieldingInteraction, b, {
                    failed, allow_indefinite_hold, display_progress, cancel_on_other_click, fail_on_damage,
                    mouse_sensitivity_adjustment_target: mst, mouse_sensitivity_adjustment_duration: msd,
                    charged_next, forks, charging_delay,
                    blocked_effects: a.obj("BlockedEffects").map(|e| damage_effects(ctx, e)).transpose()?,
                    has_modifiers: has,
                    angled_wielding: a.obj("AngledWielding").map(|w| p::AngledWielding {
                        angle_rad: w.f32_or("Angle", 0.0) * (std::f64::consts::PI / 180.0) as f32,
                        angle_distance_rad: w.f32_or("AngleDistance", 0.0) * (std::f64::consts::PI / 180.0) as f32,
                        has_modifiers: w.get("DamageModifiers").is_some() || w.get("KnockbackModifiers").is_some(),
                    }),
                })
            }
        }
        "SelectInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let hit_entity = r.root(a, "HitEntity")?;
            let hit_entity_rules = match a.get("HitEntityRules").and_then(Value::as_array) {
                Some(l) => {
                    let mut out = Vec::new();
                    for (i, h) in l.iter().enumerate() {
                        let k = h.get("Next").and_then(|x| {
                            reference(
                                x,
                                &owner_of(store, key, "HitEntityRules"),
                                &format!("HitEntityRules_{i}_Next"),
                            )
                        });
                        out.push(p::HitEntity {
                            next: r.root_key(k.as_deref())?,
                            matchers: h
                                .get("Matchers")
                                .and_then(Value::as_array)
                                .map(|m| {
                                    m.iter()
                                        .map(|x| p::EntityMatcher {
                                            r#type: match x.str_at("Type").as_deref() {
                                                Some("Vulnerable") => {
                                                    p::EntityMatcherType::VULNERABLE_MATCHER
                                                }
                                                Some("Player") => p::EntityMatcherType::PLAYER,
                                                _ => p::EntityMatcherType::SERVER,
                                            },
                                            invert: x.bool_or("Invert", false),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        });
                    }
                    Some(out)
                }
                None => None,
            };
            iv!(SelectInteraction, b, {
                next, failed,
                selector: selector(a.obj("Selector")),
                ignore_owner: a.bool_or("IgnoreOwner", true),
                hit_entity,
                hit_entity_rules,
                fail_on: enum_or(a, "FailOn", p::FailOnType::from_name, p::FailOnType::from_name("Neither").unwrap_or_default()),
            })
        }
        "DamageEntityInteraction" => {
            let (next, failed, blocked) = (r.i(a, "Next")?, r.i(a, "Failed")?, r.i(a, "Blocked")?);
            let angled_damage = match a.get("AngledDamage").and_then(Value::as_array) {
                Some(l) => {
                    let mut out = Vec::new();
                    for (i, d) in l.iter().enumerate() {
                        let k = d.get("Next").and_then(|x| {
                            reference(
                                x,
                                &owner_of(store, key, "AngledDamage"),
                                &format!("AngledDamage_{i}_Next"),
                            )
                        });
                        out.push(p::AngledDamage {
                            angle: (d.f32_or("Angle", 0.0) * (std::f64::consts::PI / 180.0) as f32)
                                as f64,
                            angle_distance: (d.f32_or("AngleDistance", 0.0)
                                * (std::f64::consts::PI / 180.0) as f32)
                                as f64,
                            damage_effects: d
                                .obj("DamageEffects")
                                .map(|e| damage_effects(ctx, e))
                                .transpose()?,
                            next: r.i_key(k)?,
                        });
                    }
                    Some(out)
                }
                None => None,
            };
            let mut targeted_damage = Vec::new();
            if let Some(m) = a.obj("TargetedDamage").and_then(Value::as_object) {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                for t in keys {
                    let d = &m[t];
                    let k = d.get("Next").and_then(|x| {
                        reference(
                            x,
                            &owner_of(store, key, "TargetedDamage"),
                            &format!("TargetedDamage_{t}_Next"),
                        )
                    });
                    targeted_damage.push((
                        t.clone(),
                        p::TargetedDamage {
                            index: 0,
                            damage_effects: d
                                .obj("DamageEffects")
                                .map(|e| damage_effects(ctx, e))
                                .transpose()?,
                            next: r.i_key(k)?,
                        },
                    ));
                }
            }
            let entity_stats_on_hit = match a.get("EntityStatsOnHit").and_then(Value::as_array) {
                Some(l) => Some(
                    l.iter()
                        .map(|s| {
                            Ok(p::EntityStatOnHit {
                                entity_stat_index: ctx.index_or(
                                    entity_stat_types::PATH,
                                    entity_stat_types::BUILTIN,
                                    s.str_at("EntityStatId").as_deref(),
                                    i32::MIN,
                                )?,
                                amount: s.f32_or("Amount", 0.0),
                                multipliers_per_entities_hit: s
                                    .get("MultipliersPerEntitiesHit")
                                    .and_then(Value::as_array)
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(Value::as_f64)
                                            .map(|x| x as f32)
                                            .collect()
                                    })
                                    .unwrap_or_else(|| vec![1.0, 0.6, 0.4, 0.2, 0.1]),
                                multiplier_per_extra_entity_hit: s
                                    .f32_or("MultiplierPerExtraEntityHit", 0.05),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                ),
                None => None,
            };
            iv!(DamageEntityInteraction, b, {
                next, failed, blocked,
                damage_effects: a.obj("DamageEffects").map(|e| damage_effects(ctx, e)).transpose()?,
                angled_damage,
                targeted_damage,
                entity_stats_on_hit,
            })
        }
        "ApplyForceInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            // Ground and collision links fall back to `Next`.
            let fallback = |r: &Refs, f: &str| match a.get(f) {
                Some(v) => r.link_key(Some(v), f, f),
                None => r.link_key(a.get("Next"), "Next", "Next"),
            };
            let gk = fallback(r, "GroundNext");
            let ground_next = r.i_key(gk)?;
            let ck = fallback(r, "CollisionNext");
            let collision_next = r.i_key(ck)?;
            let force = |f: &Value| {
                // Directions are normalized (in double precision).
                let d = f.obj("Direction").map_or([0.0, 1.0, 0.0], |d| {
                    let (x, y, z) = (d.f64_or("X", 0.0), d.f64_or("Y", 0.0), d.f64_or("Z", 0.0));
                    let l = (x * x + y * y + z * z).sqrt();
                    if l > 0.0 {
                        [(x / l) as f32, (y / l) as f32, (z / l) as f32]
                    } else {
                        [x as f32, y as f32, z as f32]
                    }
                });
                p::AppliedForce {
                    direction: d,
                    adjust_vertical: f.bool_or("AdjustVertical", false),
                    force: f.f32_or("Force", 1.0),
                }
            };
            let forces = match a.get("Forces").and_then(Value::as_array) {
                Some(l) => l.iter().map(force).collect(),
                None => vec![force(&a)],
            };
            iv!(ApplyForceInteraction, b, {
                next, failed,
                velocity_config: a.obj("VelocityConfig").map(|v| p::VelocityConfig {
                    ground_resistance: v.f32_or("GroundResistance", 0.82),
                    ground_resistance_max: v.f32_or("GroundResistanceMax", 0.0),
                    air_resistance: v.f32_or("AirResistance", 0.96),
                    air_resistance_max: v.f32_or("AirResistanceMax", 0.0),
                    threshold: v.f32_or("Threshold", 1.0),
                    style: enum_or(v, "Style", p::VelocityThresholdStyle::from_name, p::VelocityThresholdStyle::from_name("Linear").unwrap_or_default()),
                }),
                change_velocity_type: enum_or(a, "ChangeVelocityType", p::ChangeVelocityType::from_name, p::ChangeVelocityType::from_name("Set").unwrap_or_default()),
                forces,
                duration: a.f32_or("Duration", 0.0),
                vertical_clamp: a.get("VerticalClamp").and_then(Value::as_array).map(|c| {
                    let at = |i: usize| c.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32 * (std::f64::consts::PI / 180.0) as f32;
                    p::FloatRange { inclusive_min: at(0), inclusive_max: at(1) }
                }),
                wait_for_ground: a.bool_or("WaitForGround", true),
                wait_for_collision: a.bool_or("WaitForCollision", false),
                ground_check_delay: a.f32_or("GroundCheckDelay", 0.1),
                collision_check_delay: a.f32_or("CollisionCheckDelay", 0.0),
                ground_next,
                collision_next,
                raycast_distance: a.f32_or("RaycastDistance", 1.5),
                raycast_height_offset: a.f32_or("RaycastHeightOffset", 0.0),
                raycast_mode: enum_or(a, "RaycastMode", p::RaycastMode::from_name, p::RaycastMode::from_name("FollowMotion").unwrap_or_default()),
                use_target_block_for_direction: a.bool_or("UseTargetBlockForDirection", false),
            })
        }
        "BlockConditionInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(BlockConditionInteraction, b, {
                next, failed, use_latest_target: use_latest,
                matchers: a.get("Matchers").and_then(Value::as_array).map(|l| {
                    l.iter()
                        .map(|m| p::BlockMatcher {
                            block: m.obj("Block").map(|bl| p::BlockIdMatcher {
                                id: bl.str_at("Id"),
                                state: bl.str_at("State"),
                                tag_index: bl.str_at("Tag").map_or(i32::MIN, |t| ctx.tag(&t)),
                            }),
                            face: enum_or(m, "Face", p::BlockFace::from_name, p::BlockFace::from_name("None").unwrap_or_default()),
                            static_face: m.bool_or("StaticFace", false),
                        })
                        .collect()
                }),
            })
        }
        "ChangeStateInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(ChangeStateInteraction, b, {
                next, failed, use_latest_target: use_latest,
                state_changes: a.obj("Changes").and_then(Value::as_object).map(|m| {
                    m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned())).collect()
                }),
                require_block_placement: a.bool_or("RequireBlockPlacement", false),
            })
        }
        "ChangeBlockInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            let mut changes = Vec::new();
            if let Some(m) = a.obj("Changes").and_then(Value::as_object) {
                for (from, to) in m {
                    let f = ctx.index_or(
                        super::blocks::PATH,
                        super::blocks::BUILTIN,
                        Some(from),
                        i32::MIN,
                    )?;
                    let t = ctx.index_or(
                        super::blocks::PATH,
                        super::blocks::BUILTIN,
                        to.as_str(),
                        i32::MIN,
                    )?;
                    if f != i32::MIN && t != i32::MIN {
                        changes.push((f, t));
                    }
                }
            }
            iv!(ChangeBlockInteraction, b, {
                next, failed, use_latest_target: use_latest,
                block_changes: Some(changes),
                world_sound_event_index: sound_events::index(ctx, a.str_at("WorldSoundEventId").as_deref())?,
                require_not_broken: a.bool_or("RequireNotBroken", false),
            })
        }
        "ModifyInventoryInteraction" => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(ModifyInventoryInteraction, b, {
                next, failed,
                required_game_mode: None,
                item_to_remove: a.obj("ItemToRemove").map(item_stack),
                adjust_held_item_quantity: a.i32_or("AdjustHeldItemQuantity", 0),
                item_to_add: a.obj("ItemToAdd").map(item_stack),
                broken_item: a.str_at("BrokenItem"),
                adjust_held_item_durability: a.get("AdjustHeldItemDurability").and_then(Value::as_f64).unwrap_or(0.0),
            })
        }
        _ => {
            let (next, failed) = (r.i(a, "Next")?, r.i(a, "Failed")?);
            iv!(SimpleInteraction, b, { next, failed })
        }
    })
}

fn item_stack(v: &Value) -> p::ItemWithAllMetadata {
    p::ItemWithAllMetadata {
        item_id: v.str_at("Id").unwrap_or_default(),
        quantity: v.i32_or("Quantity", 1),
        durability: v.get("Durability").and_then(Value::as_f64).unwrap_or(0.0),
        max_durability: v
            .get("MaxDurability")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        quality: -1,
        override_dropped_item_animation: v.bool_or("OverrideDroppedItemAnimation", false),
        metadata: v.get("Metadata").map(|m| m.to_string()),
    }
}

/// The interaction links of a packet, in the order `interaction` records them.
pub fn packet_refs(i: &p::Interaction) -> Vec<i32> {
    use p::Interaction as I;
    match i {
        I::SimpleBlockInteraction(x) => vec![x.next, x.failed],
        I::SimpleInteraction(x) => vec![x.next, x.failed],
        I::PlaceBlockInteraction(x) => vec![x.next, x.failed],
        I::BreakBlockInteraction(x) => vec![x.next, x.failed],
        I::PickBlockInteraction(x) => vec![x.next, x.failed],
        I::UseBlockInteraction(x) => vec![x.next, x.failed],
        I::UseEntityInteraction(x) => vec![x.next, x.failed],
        I::BuilderToolInteraction(x) => vec![x.next, x.failed],
        I::ModifyInventoryInteraction(x) => vec![x.next, x.failed],
        I::ChargingInteraction(x) => {
            let mut v = vec![x.failed];
            if let Some(n) = &x.charged_next {
                let mut n = n.clone();
                n.sort_by(|a, b| a.0.total_cmp(&b.0));
                v.extend(n.iter().map(|e| e.1));
            }
            v
        }
        I::WieldingInteraction(x) => {
            let mut v = vec![x.failed];
            if let Some(n) = &x.charged_next {
                let mut n = n.clone();
                n.sort_by(|a, b| a.0.total_cmp(&b.0));
                v.extend(n.iter().map(|e| e.1));
            }
            v
        }
        I::ChainingInteraction(x) => {
            let mut v = x.chaining_next.clone();
            if let Some(f) = &x.flags {
                let mut f = f.clone();
                f.sort();
                v.extend(f.iter().map(|e| e.1));
            }
            v
        }
        I::ConditionInteraction(x) => vec![x.next, x.failed],
        I::StatsConditionInteraction(x) => vec![x.next, x.failed],
        I::BlockConditionInteraction(x) => vec![x.next, x.failed],
        I::ReplaceInteraction(_) => vec![],
        I::ChangeBlockInteraction(x) => vec![x.next, x.failed],
        I::ChangeStateInteraction(x) => vec![x.next, x.failed],
        I::FirstClickInteraction(x) => vec![x.click, x.held],
        I::SelectInteraction(x) => vec![x.next, x.failed],
        I::DamageEntityInteraction(x) => {
            let mut v = vec![x.next, x.failed, x.blocked];
            v.extend(x.angled_damage.iter().flatten().map(|d| d.next));
            let mut t = x.targeted_damage.clone();
            t.sort_by(|a, b| a.0.cmp(&b.0));
            v.extend(t.iter().map(|d| d.1.next));
            v
        }
        I::RepeatInteraction(x) => vec![x.next, x.failed],
        I::ParallelInteraction(_) => vec![],
        I::ChangeActiveSlotInteraction(_) => vec![],
        I::EffectConditionInteraction(x) => vec![x.next, x.failed],
        I::ApplyForceInteraction(x) => vec![x.next, x.failed, x.ground_next, x.collision_next],
        I::ApplyEffectInteraction(x) => vec![x.next, x.failed],
        I::ClearEntityEffectInteraction(x) => vec![x.next, x.failed],
        I::SerialInteraction(x) => x.serial_interactions.clone(),
        I::ChangeStatInteraction(x) => vec![x.next, x.failed],
        I::MovementConditionInteraction(x) => vec![
            x.next,
            x.failed,
            x.forward,
            x.back,
            x.left,
            x.right,
            x.forward_left,
            x.forward_right,
            x.back_left,
            x.back_right,
        ],
        I::ProjectileInteraction(x) => vec![x.next, x.failed],
        I::RemoveEntityInteraction(x) => vec![x.next, x.failed],
        I::ResetCooldownInteraction(x) => vec![x.next, x.failed],
        I::TriggerCooldownInteraction(x) => vec![x.next, x.failed],
        I::CooldownConditionInteraction(x) => vec![x.next, x.failed],
        I::ChainFlagInteraction(x) => vec![x.next, x.failed],
        I::IncrementCooldownInteraction(x) => vec![x.next, x.failed],
        I::CancelChainInteraction(x) => vec![x.next, x.failed],
        I::RunRootInteraction(x) => vec![x.next, x.failed],
        I::CameraInteraction(x) => vec![x.next, x.failed],
        I::SpawnDeployableFromRaycastInteraction(x) => vec![x.next, x.failed],
        I::MemoriesConditionInteraction(x) => vec![x.failed],
        I::ToggleGliderInteraction(x) => vec![x.next, x.failed],
        I::DurabilityConditionInteraction(x) => vec![x.next, x.failed],
        I::DragPlaceBlockInteraction(x) => vec![x.next, x.failed],
        I::ExtrudePlaceBlockInteraction(x) => vec![x.next, x.failed],
        I::SurfaceDrawPlaceBlockInteraction(x) => vec![x.next, x.failed],
        I::PlaceModeSelectInteraction(x) => vec![
            x.next,
            x.failed,
            x.default_interactions,
            x.replace_interactions,
            x.retype_interactions,
            x.extrude_interactions,
            x.surface_draw_interactions,
            x.fast_place_interactions,
        ],
        I::DragEraseBlockInteraction(x) => vec![x.next, x.failed],
        I::GroundedConditionInteraction(x) => vec![x.next, x.failed],
    }
}

/// One interaction by key, with its recorded links.
pub fn build_one(
    ctx: &Ctx,
    store: &Store,
    k: &str,
) -> Result<(p::Interaction, Vec<Option<String>>)> {
    let mut r = Refs {
        ctx,
        store,
        key: k,
        keys: Vec::new(),
    };
    let unknown = store
        .own
        .get(k)
        .is_some_and(|v| v.get("$Unknown").is_some());
    if !store.own.contains_key(k) || unknown {
        // Built into the server: a simple interaction waiting for nothing.
        let ty = match k {
            "*Change_Active_Slot" => "ChangeActiveSlot",
            "*Simple_Crafting_Default"
            | "*Diagram_Crafting_Default"
            | "*Structural_Crafting_Default" => "OpenBenchPage",
            "*UseNPC" => "UseNPC",
            "*PickupItem" => "PickupItem",
            _ => "Simple",
        };
        let mut i = interaction(&mut r, &Value::Null, ty)?;
        clear_tags(&mut i);
        return Ok((i, r.keys));
    }
    let a = store.resolved(k, "Interaction");
    let ty = type_of(store, k);
    let i = interaction(&mut r, &a, &ty)?;
    Ok((i, r.keys))
}

fn clear_tags(i: &mut p::Interaction) {
    macro_rules! t { ($($v:ident),*) => { match i { $(p::Interaction::$v(x) => x.tags = None,)* } } }
    t!(
        SimpleBlockInteraction,
        SimpleInteraction,
        PlaceBlockInteraction,
        BreakBlockInteraction,
        PickBlockInteraction,
        UseBlockInteraction,
        UseEntityInteraction,
        BuilderToolInteraction,
        ModifyInventoryInteraction,
        ChargingInteraction,
        WieldingInteraction,
        ChainingInteraction,
        ConditionInteraction,
        StatsConditionInteraction,
        BlockConditionInteraction,
        ReplaceInteraction,
        ChangeBlockInteraction,
        ChangeStateInteraction,
        FirstClickInteraction,
        SelectInteraction,
        DamageEntityInteraction,
        RepeatInteraction,
        ParallelInteraction,
        ChangeActiveSlotInteraction,
        EffectConditionInteraction,
        ApplyForceInteraction,
        ApplyEffectInteraction,
        ClearEntityEffectInteraction,
        SerialInteraction,
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
        MemoriesConditionInteraction,
        ToggleGliderInteraction,
        DurabilityConditionInteraction,
        DragPlaceBlockInteraction,
        ExtrudePlaceBlockInteraction,
        SurfaceDrawPlaceBlockInteraction,
        PlaceModeSelectInteraction,
        DragEraseBlockInteraction,
        GroundedConditionInteraction
    )
}

/// What the server needs to run interactions and the packets do not say:
/// one line per interaction, `index<TAB>server type<TAB>{fields}`, the
/// fields being the server-only settings the asset gives (absent ones
/// keep the server's defaults).
pub fn server_table(ctx: &Ctx) -> Result<String> {
    const FIELDS: &[&str] = &[
        "ViewDistance",
        "Tool",
        "MatchTool",
        "DefaultOk",
        "ValidateForkPositions",
        "MaxForkPositionViolations",
    ];
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    let mut out = String::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        let unknown = store
            .own
            .get(k)
            .is_some_and(|v| v.get("$Unknown").is_some());
        let (ty, a) = if !store.own.contains_key(k) || unknown {
            let ty = match k.as_str() {
                "*Change_Active_Slot" => "ChangeActiveSlot",
                "*Simple_Crafting_Default"
                | "*Diagram_Crafting_Default"
                | "*Structural_Crafting_Default" => "OpenBenchPage",
                "*UseNPC" => "UseNPC",
                "*PickupItem" => "PickupItem",
                _ => "Simple",
            };
            (ty.to_owned(), Value::Null)
        } else {
            (type_of(&store, k), store.resolved(k, "Interaction"))
        };
        let mut fields = serde_json::Map::new();
        for f in FIELDS {
            if let Some(v) = a.get(*f) {
                fields.insert((*f).to_owned(), v.clone());
            }
        }
        if let Some(w) = a
            .obj("Effects")
            .and_then(|e| e.get("WaitForAnimationToFinish"))
        {
            fields.insert("WaitForAnimationToFinish".to_owned(), w.clone());
        }
        out += &format!("{i}\t{ty}\t{}\n", Value::Object(fields));
    }
    Ok(out)
}

pub fn build(ctx: &Ctx) -> Result<p::UpdateInteractions> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, BUILTIN)?;
    let idx = ctx.indexes(PATH, BUILTIN)?;
    let mut list = Vec::new();
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        list.push((i, build_one(ctx, &store, k)?.0));
    }
    Ok(p::UpdateInteractions {
        r#type: super::INIT,
        max_id: super::max_id(&list).max(order.len() as i32),
        interactions: Some(list),
    })
}

/// Compare runs: starting from the indexes the root interactions pinned,
/// follows every link of the captured packets to pin the interactions they
/// lead to, and collects the tag lists.
pub fn pin_tags(ctx: &Ctx, captured: &[u8]) -> Result<()> {
    let (cap, _) = <p::UpdateInteractions as Wire>::decode(captured, 0)?;
    if cap.r#type != super::INIT {
        return Ok(());
    }
    let store = ctx.store(PATH)?;
    let cap: std::collections::HashMap<i32, p::Interaction> =
        cap.interactions.unwrap_or_default().into_iter().collect();
    let seed = ctx.indexes(PATH, BUILTIN)?;
    let mut lower: std::collections::HashMap<String, String> = Default::default();
    for k in store
        .own
        .keys()
        .map(String::as_str)
        .chain(BUILTIN.iter().copied())
    {
        lower.insert(k.to_lowercase(), k.to_owned());
    }
    // Pins whose key Aerie does not know (made up at run time) are kept as they are.
    let mut known: std::collections::HashMap<String, i32> = seed
        .iter()
        .filter(|(k, _)| !lower.contains_key(*k))
        .map(|(k, i)| (k.clone(), *i))
        .collect();
    // Seeds first: they come from the captured root interactions.
    let mut queue: Vec<(String, i32)> = seed
        .iter()
        .filter_map(|(k, i)| Some((lower.get(k)?.clone(), *i)))
        .collect();
    for (k, i) in &queue {
        known.insert(k.clone(), *i);
    }
    let mut done: std::collections::HashSet<String> = Default::default();
    while let Some((k, i)) = queue.pop() {
        if !done.insert(k.clone()) {
            continue;
        }
        known.entry(k.clone()).or_insert(i);
        let Some(c) = cap.get(&i) else { continue };
        let (ours, keys) = build_one(ctx, &store, &k)?;
        let refs = packet_refs(c);
        // Links line up only when both sides are the same variant.
        if std::mem::discriminant(&ours) != std::mem::discriminant(c) || keys.len() != refs.len() {
            continue;
        }
        for (key, idx) in keys.into_iter().zip(refs) {
            if let Some(key) = key {
                if idx != i32::MIN
                    && !known.contains_key(&key)
                    && lower.contains_key(&key.to_lowercase())
                {
                    known.insert(key.clone(), idx);
                    queue.push((key, idx));
                }
            }
        }
    }
    // What no chain reaches is paired by content, links and tags aside.
    let claimed: std::collections::HashSet<i32> = known.values().copied().collect();
    let mut free: std::collections::HashMap<String, Vec<i32>> = Default::default();
    let mut free_idx: Vec<&i32> = cap.keys().filter(|i| !claimed.contains(i)).collect();
    free_idx.sort();
    for i in free_idx {
        free.entry(signature(&cap[i])).or_default().push(*i);
    }
    let mut rest: Vec<&String> = store
        .own
        .keys()
        .filter(|k| !known.contains_key(*k))
        .collect();
    rest.sort();
    for k in rest {
        let (built, _) = build_one(ctx, &store, k)?;
        if let Some(list) = free.get_mut(&signature(&built)) {
            if !list.is_empty() {
                let i = list.remove(0);
                known.insert(k.clone(), i);
            }
        }
    }
    // Tags named by block matchers.
    for (k, i) in &known {
        if let (Some(p::Interaction::BlockConditionInteraction(c)), Some(a)) =
            (cap.get(i), store.own.get(k))
        {
            let json = store.resolved(k, "Interaction");
            let _ = a;
            for (m, cm) in json
                .get("Matchers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .zip(c.matchers.iter().flatten())
            {
                if let (Some(t), Some(b)) =
                    (m.obj("Block").and_then(|b| b.str_at("Tag")), &cm.block)
                {
                    ctx.pin_tag(&t, b.tag_index);
                }
            }
        }
    }
    let mut obs = Vec::new();
    for (k, i) in &known {
        if let Some(t) = cap.get(i).and_then(tags_of) {
            if store.own.contains_key(k) {
                obs.push((tags::of(&store, k).expanded, t));
            }
        }
    }
    ctx.pin(PATH, known.into_iter().collect::<Vec<_>>());
    tags::learn(ctx, &obs);
    Ok(())
}

fn tags_of(i: &p::Interaction) -> Option<Vec<i32>> {
    macro_rules! t { ($($v:ident),*) => { match i { $(p::Interaction::$v(x) => x.tags.clone(),)* } } }
    t!(
        SimpleBlockInteraction,
        SimpleInteraction,
        PlaceBlockInteraction,
        BreakBlockInteraction,
        PickBlockInteraction,
        UseBlockInteraction,
        UseEntityInteraction,
        BuilderToolInteraction,
        ModifyInventoryInteraction,
        ChargingInteraction,
        WieldingInteraction,
        ChainingInteraction,
        ConditionInteraction,
        StatsConditionInteraction,
        BlockConditionInteraction,
        ReplaceInteraction,
        ChangeBlockInteraction,
        ChangeStateInteraction,
        FirstClickInteraction,
        SelectInteraction,
        DamageEntityInteraction,
        RepeatInteraction,
        ParallelInteraction,
        ChangeActiveSlotInteraction,
        EffectConditionInteraction,
        ApplyForceInteraction,
        ApplyEffectInteraction,
        ClearEntityEffectInteraction,
        SerialInteraction,
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
        MemoriesConditionInteraction,
        ToggleGliderInteraction,
        DurabilityConditionInteraction,
        DragPlaceBlockInteraction,
        ExtrudePlaceBlockInteraction,
        SurfaceDrawPlaceBlockInteraction,
        PlaceModeSelectInteraction,
        DragEraseBlockInteraction,
        GroundedConditionInteraction
    )
}

/// A packet's debug text with interaction links, root links and tags blanked
/// (for pairing by content).
fn signature(i: &p::Interaction) -> String {
    const FIELDS: &[&str] = &[
        "next: ",
        "failed: ",
        "blocked: ",
        "click: ",
        "held: ",
        "ground_next: ",
        "collision_next: ",
        "default_interactions: ",
        "replace_interactions: ",
        "retype_interactions: ",
        "extrude_interactions: ",
        "surface_draw_interactions: ",
        "fast_place_interactions: ",
        "forward: ",
        "back: ",
        "left: ",
        "right: ",
        "forward_left: ",
        "forward_right: ",
        "back_left: ",
        "back_right: ",
        "serial_interactions: ",
        "chaining_next: ",
        "charged_next: ",
        "flags: ",
        "tags: ",
        "memories_next: ",
    ];
    // Root interaction links stay: those indexes are already the official ones
    // (a parallel interaction's `next` lists roots).
    let parallel = matches!(i, p::Interaction::ParallelInteraction(_));
    let s = format!("{i:?}");
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    'outer: while !rest.is_empty() {
        for f in FIELDS {
            if rest.starts_with(f) && !out.ends_with('_') && !(parallel && *f == "next: ") {
                out.push_str(f);
                rest = &rest[f.len()..];
                // Skip the value: a number, or a bracketed / parenthesized group.
                let mut depth = 0i32;
                let mut end = 0;
                for (j, c) in rest.char_indices() {
                    match c {
                        '[' | '(' | '{' => depth += 1,
                        ']' | ')' | '}' => {
                            if depth == 0 {
                                end = j;
                                break;
                            }
                            depth -= 1;
                        }
                        ',' if depth == 0 => {
                            end = j;
                            break;
                        }
                        _ => {}
                    }
                    end = j + c.len_utf8();
                }
                out.push('#');
                rest = &rest[end..];
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}
