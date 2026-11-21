# 07 · Registries from assets

The registries of setup (spec 05) are built from the player's own
`Assets.zip`, read at run time. Nothing from it is stored in this repository.

## Loading

- An asset folder is `Server/<path>/` inside `Assets.zip`; every `.json` file
  under it is one asset, keyed by its file name without extension.
- Files are JSON, with comments and trailing commas allowed in places (mostly
  world generation). Most folders use `.json`; particles use
  `.particlesystem` and `.particlespawner` in the same `Particles` folder.
- **Contained assets** (below): an asset can define another asset inline
  instead of naming one.
- **The schema says what inherits** (spec "Asset schema"): a field inherits
  when it has `hytale.inheritsProperty`; inside an object field the child
  sets, the object's own inheriting fields come from the parent's object.
  `Store::resolved` applies exactly that and agrees with every hand-written
  list checked so far (particle spawners included).
- **Inherited values are taken whole.** When a field inherits and the child
  sets it, the child's value replaces the parent's entirely (objects
  included); a few map fields combine entry by entry instead (an item
  animation set's `Animations`).
- **Nested inheritance.** An object field can inherit as a whole while only
  some of its own fields inherit (a particle spawner's `Particle`: when the
  child sets it, only `UseSpriteBlending` comes from the parent's).
- **Inheritance.** A `Parent` key names another asset of the same folder
  (case-insensitive). The child starts as a copy of the parent; fields the
  child sets replace the parent's, and nested objects merge field by field.
  **Only some fields inherit**, and which ones is part of each asset type:
  most do not (an audio category inherits nothing; a sound event inherits its
  volume, pitch, distances, layers and category but not its state bindings or
  ducking rules). Each registry module lists its inherited keys.
- **Indexes.** Indexed registries number their assets in this order:
  1. entries built into the server (for example `Default`, `Unknown` or
     `EMPTY` at index 0). Keys are case-insensitive: a file named like a
     built-in entry (`Empty.json`) replaces it and keeps its index;
  2. assets without a `Parent`, in reverse alphabetical order of their full
     paths inside the pack (subfolders count);
  3. assets with a `Parent`, in passes: each pass takes the waiting assets
     whose parent is already loaded, in the iteration order of the server's
     concurrent hash map of waiting assets.
  `max_id` is the number of entries. **Step 3 is not stable even on the
  official server**: two runs of the same build gave different indexes to 20
  of 1570 sound events (keys sharing a hash bucket swap). Clients only need
  indexes to agree across registries, so Aerie keeps its own deterministic
  order, and the compare tool pins the capture's indexes before comparing.
- The packet's map is index → entry. Its order on the wire is whatever the
  server's hash map produced; clients do not depend on it, so Aerie compares
  registries with captures after sorting by index.

## Asset schema

`gen/assets.json` (written by `tools/gen.py`) describes every asset type: its folder and
extension, its fields with their types and defaults, which fields inherit
from `Parent` (`hytale.inheritsProperty`), which name other assets
(`hytaleAssetRef`), and how polymorphic objects pick their variant
(`hytaleSchemaTypeField`: the discriminating field, its values in the same
order as the `anyOf` variants, and a `defaultValue` for some). It is the
official server's own schema output, condensed: start the server with
`--generate-asset-schema <dir>` (it writes `Schema/*.json` and exits), then
run `tools/asset_schema_extract.py <dir>/Schema gen/assets.json` (`tools/gen.py` does all of this).
Property order is kept: it is the order in which the server's decoder reads
fields. Built-in entries are listed per type in `hytale.internalKeys`.

## Contained assets

A field that names an asset may hold the asset itself. In the schema such a
field is an `anyOf` of a key string and the referenced type's root schema
(`{"$ref": "EntityEffect.json#"}`), with `hytaleAssetRef` naming the type.
The inline asset joins that type's assets:

- **Key:** `*` + the owner's key + `_` + the field path joined by `_` (field
  names, map keys and array indexes), so each level of nesting adds a `*`:
  `*Fire_Interactions_Collision` (a root interaction inside the fluid `Fire`),
  `**Fire_Interactions_Collision_Interactions_0` (an interaction inside it),
  `****Fire_Interactions_Collision_Interactions_0_Next_EffectId`.
- **Exceptions:** an item's `BlockType` takes the item's own key, and its
  `Parent` falls back to the item's parent. Item states (`State.<name>`) and
  block states (`State.Definitions.<name>`) inherit from their owner when
  they name no `Parent`.
- **NPC roles** are read by their own builders, not by the schema. Their
  `InteractionVars` maps (anywhere in the role) hold root interactions keyed
  by the field names on the way, without array indexes
  (`*Template_Dungeon_Goblin_Guardian_Instructions_Instructions_Actions_InteractionVars_Club_Swing_Down_Damage`);
  `Modify._InteractionVars` entries take the entry name alone
  (`*Antelope_Melee_Damage`); an inline `CombatConfig` is a balance asset.
- **Finding them:** walk every asset of every type against its schema
  (following `$ref`, picking polymorphic variants by their type field, the
  parent's type, or the default), then walk each contained asset the same
  way. A variant's `oneOf` of `required` clauses is validation, not a choice.
- **Order:** contained assets come after the type's file assets. They are
  added in batches, one per owner type and nesting level. In a batch, owners
  come in the iteration order of a `java.util.HashMap` keyed by owner key
  (bucket of `hash ^ hash >>> 16` in a table of 16 doubling past 3/4 load,
  then insertion order), inserted as the owner type loaded them: owners
  without a parent in alphabetical order of their paths, then those with
  one. Each owner's assets keep their reading order; those with a parent
  wait in passes, like file assets.

**Missing interactions.** A field that names an interaction by key (found by
the schema walk: an interaction reference holding a string) whose key does
not exist makes the official server create an empty `Simple` interaction
under that key, after all the others (`Effect`, named by a consumable).

**Checking interactions.** Interactions carry no ids. A compare run takes the
interaction indexes the captured root interactions use, then follows every
link of the captured packets (links are listed in a fixed order per variant,
and only followed when both sides are the same variant). What no link
reaches is paired by content with root links kept and interaction links and
tags blanked; interactions with identical content can still swap, which
shows up as a handful of differing links.

Batches written inside the folder's own assets come first (they load with
the folder), then those of other folders as those load. The official server
loads some folders in parallel, so batches of different folders can
interleave (music containers written in ambience effects and stingers do);
compare runs pin such registries by content when they carry no ids.

**Legacy music.** An ambience effect with the old `Music {Tracks, Volume}`
(inherited) and no `MusicContainer` gets a generated random container
`_legacy_<effect>` (shuffle, loop count 0, avoid repeat 1, resume memory
600 s, the effect's audio category) over single tracks `_legacy_<effect>_<i>`.

The installation has 20999 of them; a few registries are mostly made of
them (5739 of the 5745 root interactions; the 6 others are built into the
server: `*Default_Swap`, `*PickupItem`, `*UseNPC` and three crafting ones).

## Registries

| Packet | Folder | Built-in entries | Status |
| --- | --- | --- | --- |
| `UpdateAbilityKeywords` (94) | `Abilities/Keywords` | none | same |
| `UpdateItemQualities` (55) | `Item/Qualities` | `Default` | same |
| `UpdateEntityStatTypes` (72) | `Entity/Stats` | `Unknown` | same |
| `UpdateItemCategories` (56) | `Item/Category/CreativeLibrary` | list | same |
| `UpdateFieldcraftCategories` (58) | `Item/Category/Fieldcraft` | list | same |
| `UpdateBlockGroups` (78) | `Item/Groups` | by name | same |
| `UpdateBlockBreakingDecals` (45) | `Item/Block/BreakingDecals` | by name | same |
| `UpdateViewBobbing` (76) | `Camera/ViewBobbing` | by movement type | same |
| `UpdateCameraShake` (77) | `Camera/CameraShake` | none | same |
| `UpdateItemSoundSets` (43) | `Audio/ItemSounds` | none | same |
| `UpdateBlockSoundSets` (42) | `Item/Block/Sounds` | `EMPTY` | same |
| `UpdateSoundSets` (79) | `Audio/SoundSets` | `EMPTY` | same |
| `UpdateTrails` (48) | `Entity/Trails` | by name | same |
| `UpdateReverbEffects` (81) | `Audio/Reverb` | `EMPTY` | same |
| `UpdateEqualizerEffects` (82) | `Audio/EQ` | `EMPTY` | same |
| `UpdateAudioStates` (89) | `Audio/AudioStates` | `EMPTY` | same |
| `UpdateAudioCategories` (80) | `Audio/AudioCategories` | `EMPTY` and the five `AudioCat_*` roots | same (indexes pinned) |
| `UpdateSoundEvents` (65) | `Audio/SoundEvents` | `EMPTY` | same (indexes pinned) |
| `UpdatePhysicalMaterials` (87) | `Item/Block/PhysicalMaterials` | `EMPTY` | same |
| `UpdateFluidFX` (63) | `Item/Block/FluidFX` | `Empty` | same |
| `UpdateBlockParticleSets` (44) | `Item/Block/Particles` | by name | same |
| `UpdateModelvfxs` (53) | `Entity/ModelVFX` | none | same |
| `UpdateParticleSystems` (49) | `Particles/**.particlesystem` | by name | same |
| `UpdateParticleSpawners` (50) | `Particles/**.particlespawner` | by name | same |
| `UpdateResourceTypes` (59) | `Item/ResourceTypes` | by name | same |
| `UpdateTagPatterns` (84) | `TagPatterns` | none | same (11 files, 185 contained) |
| `UpdateItemPlayerAnimations` (52) | `Item/Animations` | by name | same (160 files, 22 contained) |
| `UpdateEntityEffects` (51) | `Entity/Effects` | none | same (indexes pinned; 35 contained) |
| `UpdateHitboxCollisionConfig` (74) | `Entity/HitboxCollision` | none | same |
| `UpdateRepulsionConfig` (75) | `Entity/Repulsion` | none | same |
| `UpdateBeamConfigs` (93) | `Entity/Beams` | none | same |
| `UpdateEmotes` (86) | `Emote` (empty) | none | same |
| `UpdateEntityUIComponents` (73) | `Entity/UI` | none | same |
| `UpdateItemReticles` (57) | `Item/Reticles` | `Default` | same (server events keyed by tag index) |
| `UpdateMusicContainers` (88) | `Audio/MusicContainers` | `Empty` | same (275 files, 398 contained, 2 legacy; indexes pinned by content) |
| `UpdateStingers` (92) | `Audio/Stingers` | `EMPTY` | same |
| `UpdateMusicTransitions` (91) | `Audio/MusicTransitions` | `EMPTY` | same |
| `UpdateEnvironments` (61) | `Environments` | `Unknown` | same (indexes and tags pinned) |
| `UpdateWeathers` (47) | `Weathers` | `Unknown` | same (indexes and tags pinned) |
| `UpdateFluids` (83) | `Item/Block/Fluids` | `Empty`, `Unknown` | same (indexes and tags pinned) |
| `UpdateBlockHitboxes` (41) | `Item/Block/Hitboxes` | `Full` | same |
| `UpdateAmbienceFX` (62) | `Audio/AmbienceFX` | `Empty` | same (indexes pinned) |
| `UpdateRecipes` (60) | `Item/Recipes` + item `Recipe`s | by id | same (item tags pinned) |
| `UpdateRootInteractions` (67) | `Item/RootInteractions` | 6 built in | same (indexes pinned; the interaction indexes it uses are pinned from it) |
| `UpdateBlockTypes` (40) | `Item/Block/Blocks` + blocks in items, states | 4 built in | 5528 of 6154 the same; 622 differ only in the order of their tag lists (the sets match), 4 in state cube textures |
| `UpdateInteractions` (66) | `Item/Interactions` | 6 built in | 9138 of 9195 the same; the rest are links between interactions with identical content (see below) |
| `UpdateUnarmedInteractions` (68) | `Item/Unarmed/Interactions` (`Empty`) | none | same |
| `UpdateTranslations` (64) | `Languages/en-US/**.lang` + bench and fieldcraft names | none | same |
| `UpdateItems` (54) | `Item/Items` + item states | `Unknown` | 3761 of 4012 the same; the rest differ only in the order of their tag lists (the sets match) |
| `UpdateBlockSets` (46) | `Item/Block/Sets` | by name | same (block lists compared as sets) |
| `UpdateProjectileConfigs` (85) | `ProjectileConfigs` | by name | same |
| `UpdateConnectedBlockRuleSets` (90) | `Item/ConnectedBlockRuleSets` | by name | same |

Tag list order: every set is right, and the order is right when the
indexes are (checked by hand on small lists). Names that always appear
together (`Oak` and `Family=Oak`) can only be told apart by order, and for
some lists no pairing reproduces every captured order, so an insertion order
is still wrong somewhere (suspect: deep parent chains, where each level
re-reads its parent's raw tags in `HashMap(0)` order). `tags::joint`
searches all groups together but is off until that is found. The client most likely reads
these lists as sets (not checked).

Values the server fills in after loading (an item's unarmed interaction
fallbacks, a block's interaction hint, `Support` and `Supporting`) are
inherited already filled in: a child takes its parent's final value, and when
nothing in the chain writes the field the default comes from the topmost
ancestor. Keys are read in file order, so when a file writes both an
all-faces key (`BiomeTint`) and a per-face one (`BiomeTintUp`), the later one
wins. Weapon damage breakdowns walk the item's `Primary` and `Ability1`
chains (`registries/weapon_damage.rs`); projectile hits add nothing, because
the server computes this before projectile configs load.

"Same" means equal to the official server's packet once entries are sorted.
Registries refer to each other by index (for example a stat's effects name a
sound event, sent as its index in the sound event registry).

## Values

- Colors: `#rgb`, `#rrggbb` (4 and 8 digits accepted, alpha ignored) or
  `rgb(r, g, b)`. Colors with alpha: hex with 4 or 8 digits (3 and 6 give
  alpha 255), `rgba(#hex, a)` or `rgba(r, g, b, a)`; alpha is `a × 255`
  clamped and truncated.
- Vectors are `{"X", "Y", "Z"}`, rotations `{"Yaw", "Pitch", "Roll"}`; missing
  components are 0.
- Enums are written by name, the same names as the protocol's.
- Some audio values are written in decibels and sent as linear gain:
  `10^(dB / 20)`, 0 at -100 dB and below.
- **Tags** (`tagIndexes`, `tags` and similar fields) are indexes in a table
  the server builds as asset tags first appear; the client never receives the
  names. Like hash-ordered indexes, they only need to agree across packets.
  An asset's `"Tags": {"Material": ["Solid", "Stone"], "Ore": []}` expands to
  `Material`, `Solid`, `Material=Solid`, `Stone`, `Material=Stone` and `Ore`.
  An asset with a parent first takes the parent's raw tags (every expanded
  name with its values, walked in the order of a `java.util.HashMap` created
  with capacity 0), then adds its own `Tags` (walked in the order of a
  default `HashMap`). A packet's tag list is the set of expanded indexes in
  the iteration order of a fastutil `IntOpenHashSet` (0 first, then table
  slots from the top; `mix(k) = h ^ h >>> 16` with `h = k * 0x9E3779B9`,
  linear probing, load factor 0.75, starting at 2 slots and rehashing from
  the top slot down). `crates/aerie-assets/src/tags.rs` has all of it.
- A compare run learns the official tag indexes from captures: from the tag
  patterns (pattern i of the capture is Aerie's pattern i), from item
  reticle server events, and by solving constraints over captured tag
  lists (a name's index is in every list of an asset with the name and in
  none of one without it), once over every captured packet. Names that
  always appear together are told apart by list order (colliding slots keep
  insertion order); if nothing tells them apart they are paired in order,
  which gives the same packets.

Each registry's field mapping (JSON key → packet field, defaults) is written at
the top of its module in `crates/aerie-assets/src/registries/`.

## Checking

```
aerie-server probe --upstream 127.0.0.1:5521 --capture <dir>
cargo run --release -p aerie-assets --example compare -- <dir>
```
