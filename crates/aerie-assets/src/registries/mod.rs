//! Registries built from assets, one module each (spec 07, "Registries").

pub mod ability_keywords;
pub mod ambience;
pub mod audio_effects;
pub mod audio_states;
pub mod benches;
pub mod block_groups;
pub mod block_hitboxes;
pub mod block_sets;
pub mod blocks;
pub mod camera;
pub mod camera_shake;
pub mod connected_rule_sets;
pub mod consume;
pub mod entity_configs;
pub mod entity_effects;
pub mod entity_stat_types;
pub mod environments;
pub mod fluids;
pub mod gathering;
pub mod interactions;
pub mod item_animations;
pub mod item_categories;
pub mod item_qualities;
pub mod item_reticles;
pub mod items;
pub mod materials;
pub mod model_particles;
pub mod models;
pub mod music;
pub mod particle_spawners;
pub mod particles;
pub mod physics;
pub mod projectile_configs;
pub mod recipes;
pub mod resource_types;
pub mod root_interactions;
pub mod sound_events;
pub mod sound_sets;
pub mod tag_patterns;
pub mod trails;
pub mod translations;
pub mod view_bobbing;
pub mod visual;
pub mod weapon_damage;
pub mod weathers;
pub mod world_config;

use aerie_protocol::proto::{self, UpdateType};
use aerie_protocol::wire::{Canon, Wire};

use crate::{Result, ctx::Ctx};

/// `UpdateType` of the full list sent during setup.
pub const INIT: UpdateType = UpdateType::INIT;

/// A registry Aerie can build: its packet id, a builder, and a canonical text
/// form (entries sorted) used to compare with captured traffic.
pub struct Registry {
    pub id: u32,
    pub name: &'static str,
    /// Builds the packet payload.
    pub build: fn(&Ctx) -> Result<Vec<u8>>,
    /// Canonical text of a payload (built or captured).
    pub canonical: fn(&[u8]) -> Result<String>,
    /// For registries whose official index order is not stable: the folder,
    /// and the (key, index) pairs read from a captured payload.
    pub pin: Option<(&'static str, fn(&[u8]) -> Result<Vec<(String, i32)>>)>,
    /// Learns official tag indexes, or other indexes that cannot be pinned by
    /// key, from a captured payload (compare runs only).
    pub pin_tags: Option<fn(&Ctx, &[u8]) -> Result<()>>,
}

/// Lists without keys are put in `id` order; maps are sorted by the
/// generated `canon()`.
trait SortList {
    fn sort_list(&mut self);
}

/// Block sets are hash sets on the official server: compared as sets.
impl SortList for Option<Vec<(String, proto::BlockSet)>> {
    fn sort_list(&mut self) {
        for (_, s) in self.iter_mut().flatten() {
            if let Some(b) = &mut s.blocks {
                b.sort();
            }
        }
    }
}

impl SortList for Option<Vec<proto::ItemCategory>> {
    fn sort_list(&mut self) {
        if let Some(v) = self {
            v.sort_by(|a, b| a.id.cmp(&b.id));
        }
    }
}

macro_rules! registry {
    ($id:expr, $ty:ident, $build:path, pin $map:ident @ $path:expr) => {{
        let mut r = registry!($id, $ty, $build);
        r.pin = Some(($path, |bytes| {
            let (p, _) = <proto::$ty as Wire>::decode(bytes, 0)?;
            Ok(p.$map.unwrap_or_default().into_iter().filter_map(|(i, e)| Some((e.id?, i))).collect())
        }));
        r
    }};
    ($id:expr, $ty:ident, $build:path $(, list $list:ident)?) => {
        Registry {
            id: $id,
            name: stringify!($ty),
            build: |ctx| {
                let mut out = Vec::new();
                $build(ctx)?.encode(&mut out)?;
                Ok(out)
            },
            canonical: |bytes| {
                let (mut p, _) = <proto::$ty as Wire>::decode(bytes, 0)?;
                p.canon();
                $(p.$list.sort_list();)?
                Ok(format!("{p:#?}"))
            },
            pin: None,
            pin_tags: None,
        }
    };
}

pub fn all() -> Vec<Registry> {
    vec![
        registry!(94, UpdateAbilityKeywords, ability_keywords::build),
        registry!(55, UpdateItemQualities, item_qualities::build),
        registry!(72, UpdateEntityStatTypes, entity_stat_types::build),
        registry!(56, UpdateItemCategories, item_categories::build, list item_categories),
        registry!(58, UpdateFieldcraftCategories, item_categories::build_fieldcraft, list item_categories),
        registry!(78, UpdateBlockGroups, block_groups::build),
        registry!(45, UpdateBlockBreakingDecals, block_groups::build_decals),
        registry!(76, UpdateViewBobbing, view_bobbing::build),
        registry!(77, UpdateCameraShake, camera_shake::build),
        registry!(43, UpdateItemSoundSets, sound_sets::build_item),
        registry!(42, UpdateBlockSoundSets, sound_sets::build_block),
        registry!(79, UpdateSoundSets, sound_sets::build_set),
        registry!(65, UpdateSoundEvents, sound_events::build, pin sound_events @ sound_events::PATH),
        registry!(48, UpdateTrails, trails::build),
        registry!(81, UpdateReverbEffects, audio_effects::build_reverb),
        registry!(82, UpdateEqualizerEffects, audio_effects::build_eq),
        registry!(87, UpdatePhysicalMaterials, materials::build_materials),
        registry!(63, UpdateFluidFX, materials::build_fluid_fx),
        registry!(44, UpdateBlockParticleSets, visual::build_particle_sets),
        registry!(53, UpdateModelvfxs, visual::build_model_vfx),
        registry!(49, UpdateParticleSystems, particles::build_systems),
        registry!(50, UpdateParticleSpawners, particle_spawners::build),
        registry!(59, UpdateResourceTypes, resource_types::build),
        registry!(52, UpdateItemPlayerAnimations, item_animations::build),
        {
            let mut r = registry!(84, UpdateTagPatterns, tag_patterns::build);
            r.pin_tags = Some(tag_patterns::pin_tags);
            r
        },
        registry!(89, UpdateAudioStates, audio_states::build_states),
        registry!(80, UpdateAudioCategories, audio_states::build_categories, pin categories @ audio_states::CATEGORIES),
        registry!(
            74,
            UpdateHitboxCollisionConfig,
            entity_configs::build_hitbox
        ),
        registry!(75, UpdateRepulsionConfig, entity_configs::build_repulsion),
        registry!(93, UpdateBeamConfigs, entity_configs::build_beams),
        registry!(86, UpdateEmotes, entity_configs::build_emotes),
        registry!(73, UpdateEntityUIComponents, entity_configs::build_ui),
        {
            let mut r = registry!(57, UpdateItemReticles, item_reticles::build);
            r.pin_tags = Some(item_reticles::pin_tags);
            r
        },
        {
            let mut r = registry!(88, UpdateMusicContainers, music::build_containers);
            r.pin_tags = Some(music::pin_containers);
            r
        },
        registry!(92, UpdateStingers, music::build_stingers, pin stingers @ music::STINGERS),
        registry!(91, UpdateMusicTransitions, music::build_transitions, pin music_transitions @ music::TRANSITIONS),
        {
            let mut r = registry!(61, UpdateEnvironments, environments::build, pin environments @ environments::PATH);
            r.pin_tags = Some(environments::pin_tags);
            r
        },
        {
            let mut r =
                registry!(47, UpdateWeathers, weathers::build, pin weathers @ weathers::PATH);
            r.pin_tags = Some(weathers::pin_tags);
            r
        },
        {
            let mut r = registry!(83, UpdateFluids, fluids::build, pin fluids @ fluids::PATH);
            r.pin_tags = Some(fluids::pin_tags);
            r
        },
        registry!(41, UpdateBlockHitboxes, block_hitboxes::build),
        registry!(62, UpdateAmbienceFX, ambience::build, pin ambience_fx @ ambience::PATH),
        registry!(64, UpdateTranslations, translations::build),
        {
            let mut r = registry!(60, UpdateRecipes, recipes::build);
            r.pin_tags = Some(recipes::pin_tags);
            r
        },
        {
            let mut r = registry!(67, UpdateRootInteractions, root_interactions::build, pin interactions @ root_interactions::PATH);
            r.pin_tags = Some(root_interactions::pin_tags);
            r
        },
        {
            let mut r = registry!(66, UpdateInteractions, interactions::build);
            r.pin_tags = Some(interactions::pin_tags);
            r
        },
        registry!(
            68,
            UpdateUnarmedInteractions,
            root_interactions::build_unarmed
        ),
        {
            let mut r = registry!(40, UpdateBlockTypes, blocks::build);
            r.pin_tags = Some(blocks::pin_tags);
            r
        },
        {
            let mut r = registry!(54, UpdateItems, items::build);
            r.pin_tags = Some(items::pin_tags);
            r
        },
        registry!(90, UpdateConnectedBlockRuleSets, connected_rule_sets::build),
        registry!(85, UpdateProjectileConfigs, projectile_configs::build),
        registry!(46, UpdateBlockSets, block_sets::build, list block_sets),
        registry!(51, UpdateEntityEffects, entity_effects::build, pin entity_effects @ entity_effects::PATH),
    ]
}

/// Uses a capture's indexes and tags where the official order cannot be
/// reproduced (compare runs, and test runs that mix built and captured
/// registries). `captured` returns a registry packet's payload by id.
pub fn pin_from_capture(ctx: &Ctx, captured: impl Fn(u32) -> Option<Vec<u8>>) -> Result<()> {
    // Block types are not built yet, but other registries use their indexes.
    if let Some(b) = captured(40) {
        let (p, _) = <proto::UpdateBlockTypes as Wire>::decode(&b, 0)?;
        ctx.pin(
            blocks::PATH,
            p.block_types
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(i, b)| Some((b.name?, i))),
        );
    }
    let regs = all();
    for reg in &regs {
        if let (Some((path, pairs)), Some(b)) = (reg.pin, captured(reg.id)) {
            ctx.pin(path, pairs(&b)?);
        }
    }
    for reg in &regs {
        if let (Some(pin), Some(b)) = (reg.pin_tags, captured(reg.id)) {
            pin(ctx, &b)?;
        }
    }
    crate::tags::solve(ctx);
    Ok(())
}

/// `max_id` of an indexed registry: one past the highest index, at least
/// the number of entries (pinned indexes can leave gaps).
pub fn max_id<T>(list: &[(i32, T)]) -> i32 {
    list.iter()
        .map(|(i, _)| i + 1)
        .max()
        .unwrap_or(0)
        .max(list.len() as i32)
}

/// Every registry Aerie builds, as (packet id, payload), translations last.
pub fn build_all(ctx: &Ctx) -> Vec<(u32, &'static str, Result<Vec<u8>>)> {
    let mut regs = all();
    regs.sort_by_key(|r| r.id == 64);
    regs.into_iter()
        .map(|r| (r.id, r.name, (r.build)(ctx)))
        .collect()
}
