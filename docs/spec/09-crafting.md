# 09 — Crafting and using items

What the server does for pocket crafting, crafting benches and items used
up by an interaction (eating). Checked against a local official server
with the probe (`--chat`, `--send id:hex`).

## Windows

| Packet | Id | Way | Fields |
| --- | --- | --- | --- |
| `OpenWindow` | 200 | S→C | `id`, `window_type`, `window_data` (JSON text), `inventory`, `extra_resources` |
| `UpdateWindow` | 201 | S→C | `id`, `window_data`, `inventory`, `extra_resources` |
| `CloseWindow` | 202 | both | `id` |
| `SendWindowAction` | 203 | C→S | `id`, `action` (`CraftRecipeAction` is variant 0: `recipe_id`, `quantity`) |
| `ClientOpenWindow` | 204 | C→S | `type` (`WindowType`, one byte) |
| `SetPage` | 216 | S→C | `page` (`Bench` 1), `can_close_through_interaction` |

`WindowType`: Container 0, PocketCrafting 1, BasicCrafting 2,
DiagramCrafting 3, StructuralCrafting 4, Processing 5, Memories 6,
AbilityBench 7. `BenchType`: Crafting 0, Processing 1, DiagramCrafting 2,
StructuralCrafting 3.

### Pocket crafting (Tab)

The client sends `ClientOpenWindow{type: PocketCrafting}`. The window the
client opens itself always has id 0, and the server answers with
`UpdateWindow` (not `OpenWindow`), no inventory, no extra resources, and:

```json
{"type":0,"id":"Fieldcraft","name":"client.inventory.fieldcraft.title","categories":[],"worldMemoriesLevel":1}
```

The category list is empty on the official server (captured). The world
memories level is 1 until memories are recorded (`MemoriesAmountPerLevel`
of the gameplay config: level `i + 2` once the count reaches entry `i`).

### Benches

A block with a `Bench` (in its `BlockType`) gets a `Use` interaction when
it has none: `*Simple_Crafting_Default` (Crafting), `*Diagram_Crafting_Default`
or `*Structural_Crafting_Default` (spec 07). Using it opens the bench: a 2D
sound (`LocalOpenSoundEventId`, UI category), `SetPage{Bench, true}`, then
`OpenWindow` with the next window id of the player (from 1), the window type
of the bench type, extra resources present (empty) and data keyed in this
order:

`type` (bench type), `id` (bench id), `name` (the item's translation key),
`blockItemId`, `tierLevel` (1), `grantedAugmentTags` (`[]`), `categories`
(per bench category: `id`, `name`, `icon`, `craftableRecipes`),
`worldMemoriesLevel`, `nearbyChestCount`, `maxChestCount` (100),
`chestHorizontalRadius` (14), `chestVerticalRadius` (6), `memoriesPerLevel`
(the gameplay config's `MemoriesAmountPerLevel`).

`craftableRecipes` of a category: the recipes with a bench requirement
naming this bench id and listing the category.

Closing: the client sends `CloseWindow`; the server sends `CloseWindow`
back and plays `LocalCloseSoundEventId`.

## Crafting a recipe

`SendWindowAction{id, CraftRecipeAction{recipe_id, quantity}}`. An unknown
recipe id disconnects the player (`server.general.disconnect.unknownRecipe`).
Otherwise the recipe is made when:

- one of its bench requirements has the open bench's type and id
  (`Fieldcraft` for the pocket window), a required tier at most the
  bench's tier and only augment tags the bench grants;
- it needs no knowledge the player lacks, and the world memories level is
  at least `RequiredMemoriesLevel`;
- outside creative, the inputs are all there. They are taken from
  backpack, storage, then hotbar, slot by slot: an `ItemId` input takes
  that item; a `ResourceTypeId` input takes items whose resource types
  include it, each giving its resource quantity (items needed =
  ceil(left / quantity)); an `ItemTag` input takes items with that tag.

The outputs (`quantity` times) go into the inventory; what does not fit
is dropped. The client then gets `UpdatePlayerInventory` and, for the
pocket window, the 2D sound `SFX_Player_Craft_Item_Inventory` (UI). A bench
plays its `CompletedSoundEventId` where it stands. Missing inputs give a
`Notification` (`server.general.crafting.missingIngredient`).

Recipes with `TimeSeconds` above 0 are queued at benches on the official
server, with progress in the window data; Aerie makes them at once for
now.

## Items used up by an interaction

Food, potions and bandages: the item's `Interactions` root (Secondary for
food) leads through `Condition` (food requires Adventure mode), `Replace`
(the item's `InteractionVars`, else the default) and `Charging` to a
`ModifyInventory` with `AdjustHeldItemQuantity` (-1). A `Charging` step
goes to the `Next` entry with the largest key not above the charge value
the client reports in its sync data (`charge_value`; -1 still charging,
-2 cancelled). So the held stack changes once the client reports a charge
at least the branch's key (2.5 s for most food).

Aerie follows each such chain by id from its first sync (held item and
hotbar slot) and changes the stack once. The effects (`ApplyEffect`, e.g.
`Food_Instant_Heal_T1`) are not applied yet.
