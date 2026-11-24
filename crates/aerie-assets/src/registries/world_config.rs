//! World settings sent to a player after `JoinWorld`, from the gameplay
//! configuration asset (`Server/GameplayConfigs`, the one named `Default`,
//! else the first). Missing fields take the schema defaults
//! (`gen/assets.json`, `GameplayConfig`).
//!
//! | JSON | Packet |
//! | --- | --- |
//! | `World.AllowBlockBreaking`, `AllowBlockGathering`, `AllowBlockPlacement` (true) | `WorldConfig` same |
//! | `World.BlockBreakingBypassTag` | `block_breaking_bypass_tag_index` (tag index; `i32::MIN` absent) |
//! | `World.BlockedRootInteractionTags` | `blocked_root_interaction_tag_indexes` (empty) |
//! | `Combat.DisplayHealthBars`, `DisplayCombatText` (true) | `CombatConfig` same |
//! | `Player.ArmorVisibilityOption` (`All`) | `PlayerConfig` |
//! | `World.DaytimeDurationSeconds` (1728), `NighttimeDurationSeconds` (1151), `TotalMoonPhases` (5) | `UpdateTimeSettings` |

use aerie_protocol::proto::{
    ArmorVisibilityOption, CombatConfig, FlyMode, MovementSettings, PlayerConfig,
    UpdateTimeSettings, WorldConfig, WorldGameplayConfig,
};
use serde_json::Value;

use crate::Result;
use crate::ctx::{Ctx, enum_or};
use crate::values::Get;

pub const PATH: &str = "GameplayConfigs";
pub const MOVEMENT: &str = "Entity/MovementConfig";

/// The resolved gameplay configuration, or `Null` when there is none.
pub fn config(ctx: &Ctx) -> Result<Value> {
    let store = ctx.store(PATH)?;
    let mut keys: Vec<&String> = store.own.keys().collect();
    keys.sort();
    let key = keys
        .iter()
        .find(|k| k.eq_ignore_ascii_case("Default"))
        .or(keys.first())
        .map(|k| (*k).clone());
    Ok(match key {
        Some(k) => store.resolved(&k, "GameplayConfig"),
        None => Value::Null,
    })
}

/// The names of every gameplay configuration asset, sorted.
pub fn names(ctx: &Ctx) -> Result<Vec<String>> {
    let store = ctx.store(PATH)?;
    let mut keys: Vec<String> = store.own.keys().cloned().collect();
    keys.sort();
    Ok(keys)
}

/// One gameplay configuration by name (a world can be set to any of them),
/// resolved; `Null` when there is none of that name.
pub fn config_named(ctx: &Ctx, name: &str) -> Result<Value> {
    let store = ctx.store(PATH)?;
    Ok(
        match store.own.keys().find(|k| k.eq_ignore_ascii_case(name)) {
            Some(k) => store.resolved(k, "GameplayConfig"),
            None => Value::Null,
        },
    )
}

fn section<'a>(g: &'a Value, name: &str) -> &'a Value {
    g.get(name).unwrap_or(&Value::Null)
}

pub fn gameplay(ctx: &Ctx, g: &Value) -> WorldGameplayConfig {
    let w = section(g, "World");
    let c = section(g, "Combat");
    let p = section(g, "Player");
    WorldGameplayConfig {
        world_config: WorldConfig {
            allow_block_breaking: w.bool_or("AllowBlockBreaking", true),
            block_breaking_bypass_tag_index: w
                .str_at("BlockBreakingBypassTag")
                .map_or(i32::MIN, |t| ctx.tag(&t)),
            allow_block_gathering: w.bool_or("AllowBlockGathering", true),
            allow_block_placement: w.bool_or("AllowBlockPlacement", true),
            blocked_root_interaction_tag_indexes: Some(
                w.get("BlockedRootInteractionTags")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|t| ctx.tag(t))
                    .collect(),
            ),
        },
        combat_config: CombatConfig {
            display_health_bars: c.bool_or("DisplayHealthBars", true),
            display_combat_text: c.bool_or("DisplayCombatText", true),
        },
        player_config: PlayerConfig {
            armor_visibility_option: enum_or(
                p,
                "ArmorVisibilityOption",
                ArmorVisibilityOption::from_name,
                ArmorVisibilityOption::ALL,
            ),
        },
    }
}

pub fn time(g: &Value) -> UpdateTimeSettings {
    let w = section(g, "World");
    UpdateTimeSettings {
        daytime_duration_seconds: w.i32_or("DaytimeDurationSeconds", 1728),
        nighttime_duration_seconds: w.i32_or("NighttimeDurationSeconds", 1151),
        total_moon_phases: w.i32_or("TotalMoonPhases", 5) as i8,
        time_paused: false,
    }
}

/// The player's movement settings: the movement configuration the gameplay
/// configuration names (`Player.MovementConfig`, an asset of
/// `Entity/MovementConfig`; absent fields are 0, as the official asset
/// fields start), with mass, drag and inverted gravity from the `Player`
/// model's `PhysicsValues` (schema default 68, 0.5, false), and `Fly` from
/// the configuration, else allowed in creative and disabled otherwise.
/// `None` when the named configuration is not an asset (the built-in
/// `BuiltinDefault`).
pub fn movement(ctx: &Ctx, g: &Value, creative: bool) -> Result<Option<MovementSettings>> {
    let name = section(g, "Player")
        .str_at("MovementConfig")
        .unwrap_or_else(|| "BuiltinDefault".into());
    let store = ctx.store(MOVEMENT)?;
    let Some(k) = store
        .own
        .keys()
        .find(|k| k.eq_ignore_ascii_case(&name))
        .cloned()
    else {
        return Ok(None);
    };
    let c = store.resolved(&k, "MovementConfig");
    let models = ctx.store(super::models::MODELS)?;
    let physics = models
        .own
        .keys()
        .find(|k| k.eq_ignore_ascii_case("Player"))
        .map(|k| models.resolved(k, "ModelAsset"))
        .and_then(|m| m.get("PhysicsValues").cloned())
        .unwrap_or(Value::Null);
    let c = &c;
    Ok(Some(MovementSettings {
        mass: physics.f32_or("Mass", 68.0),
        drag_coefficient: physics.f32_or("DragCoefficient", 0.5),
        inverted_gravity: physics.bool_or("InvertedGravity", false),
        fly: match c.str_at("Fly") {
            Some(f) => FlyMode::from_name(&f).unwrap_or(FlyMode::DISABLED),
            None if creative => FlyMode::ALLOWED,
            None => FlyMode::DISABLED,
        },
        velocity_resistance: c.f32_or("VelocityResistance", 0.0),
        jump_force: c.f32_or("JumpForce", 0.0),
        swim_jump_force: c.f32_or("SwimJumpForce", 0.0),
        jump_buffer_duration: c.f32_or("JumpBufferDuration", 0.0),
        jump_buffer_max_y_velocity: c.f32_or("JumpBufferMaxYVelocity", 0.0),
        acceleration: c.f32_or("Acceleration", 0.0),
        air_drag_min: c.f32_or("AirDragMin", 0.0),
        air_drag_max: c.f32_or("AirDragMax", 0.0),
        air_drag_min_speed: c.f32_or("AirDragMinSpeed", 0.0),
        air_drag_max_speed: c.f32_or("AirDragMaxSpeed", 0.0),
        air_friction_min: c.f32_or("AirFrictionMin", 0.0),
        air_friction_max: c.f32_or("AirFrictionMax", 0.0),
        air_friction_min_speed: c.f32_or("AirFrictionMinSpeed", 0.0),
        air_friction_max_speed: c.f32_or("AirFrictionMaxSpeed", 0.0),
        air_speed_multiplier: c.f32_or("AirSpeedMultiplier", 0.0),
        air_control_min_speed: c.f32_or("AirControlMinSpeed", 0.0),
        air_control_max_speed: c.f32_or("AirControlMaxSpeed", 0.0),
        air_control_min_multiplier: c.f32_or("AirControlMinMultiplier", 0.0),
        air_control_max_multiplier: c.f32_or("AirControlMaxMultiplier", 0.0),
        combo_air_speed_multiplier: c.f32_or("ComboAirSpeedMultiplier", 0.0),
        base_speed: c.f32_or("BaseSpeed", 0.0),
        climb_speed: c.f32_or("ClimbSpeed", 0.0),
        climb_speed_lateral: c.f32_or("ClimbSpeedLateral", 0.0),
        climb_up_sprint_speed: c.f32_or("ClimbUpSprintSpeed", 0.0),
        climb_down_sprint_speed: c.f32_or("ClimbDownSprintSpeed", 0.0),
        horizontal_fly_speed: c.f32_or("HorizontalFlySpeed", 0.0),
        vertical_fly_speed: c.f32_or("VerticalFlySpeed", 0.0),
        max_speed_multiplier: c.f32_or("MaxSpeedMultiplier", 0.0),
        min_speed_multiplier: c.f32_or("MinSpeedMultiplier", 0.0),
        wish_direction_gravity_x: c.f32_or("WishDirectionGravityX", 0.0),
        wish_direction_gravity_y: c.f32_or("WishDirectionGravityY", 0.0),
        wish_direction_weight_x: c.f32_or("WishDirectionWeightX", 0.0),
        wish_direction_weight_y: c.f32_or("WishDirectionWeightY", 0.0),
        collision_expulsion_force: c.f32_or("CollisionExpulsionForce", 0.0),
        forward_walk_speed_multiplier: c.f32_or("ForwardWalkSpeedMultiplier", 0.0),
        backward_walk_speed_multiplier: c.f32_or("BackwardWalkSpeedMultiplier", 0.0),
        strafe_walk_speed_multiplier: c.f32_or("StrafeWalkSpeedMultiplier", 0.0),
        forward_run_speed_multiplier: c.f32_or("ForwardRunSpeedMultiplier", 0.0),
        backward_run_speed_multiplier: c.f32_or("BackwardRunSpeedMultiplier", 0.0),
        strafe_run_speed_multiplier: c.f32_or("StrafeRunSpeedMultiplier", 0.0),
        forward_crouch_speed_multiplier: c.f32_or("ForwardCrouchSpeedMultiplier", 0.0),
        backward_crouch_speed_multiplier: c.f32_or("BackwardCrouchSpeedMultiplier", 0.0),
        strafe_crouch_speed_multiplier: c.f32_or("StrafeCrouchSpeedMultiplier", 0.0),
        forward_sprint_speed_multiplier: c.f32_or("ForwardSprintSpeedMultiplier", 0.0),
        variable_jump_fall_force: c.f32_or("VariableJumpFallForce", 0.0),
        fall_effect_duration: c.f32_or("FallEffectDuration", 0.0),
        fall_jump_force: c.f32_or("FallJumpForce", 0.0),
        fall_momentum_loss: c.f32_or("FallMomentumLoss", 0.0),
        auto_jump_obstacle_speed_loss: c.f32_or("AutoJumpObstacleSpeedLoss", 0.0),
        auto_jump_obstacle_sprint_speed_loss: c.f32_or("AutoJumpObstacleSprintSpeedLoss", 0.0),
        auto_jump_obstacle_effect_duration: c.f32_or("AutoJumpObstacleEffectDuration", 0.0),
        auto_jump_obstacle_sprint_effect_duration: c
            .f32_or("AutoJumpObstacleSprintEffectDuration", 0.0),
        auto_jump_obstacle_max_angle: c.f32_or("AutoJumpObstacleMaxAngle", 0.0),
        auto_jump_disable_jumping: c.bool_or("AutoJumpDisableJumping", false),
        min_slide_entry_speed: c.f32_or("MinSlideEntrySpeed", 0.0),
        slide_exit_speed: c.f32_or("SlideExitSpeed", 0.0),
        min_fall_speed_to_engage_roll: c.f32_or("MinFallSpeedToEngageRoll", 0.0),
        max_fall_speed_to_engage_roll: c.f32_or("MaxFallSpeedToEngageRoll", 0.0),
        roll_start_speed_modifier: c.f32_or("RollStartSpeedModifier", 0.0),
        roll_exit_speed_modifier: c.f32_or("RollExitSpeedModifier", 0.0),
        roll_time_to_complete: c.f32_or("RollTimeToComplete", 0.0),
        max_slope_angle_degrees: c.f32_or("MaxSlopeAngleDegrees", 0.0),
        max_wall_angle_degrees: c.f32_or("MaxWallAngleDegrees", 0.0),
    }))
}
