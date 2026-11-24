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

### Queue (timed recipes, structural benches)

The pocket window always crafts at once. A `Crafting` bench crafts at once
when the recipe's `TimeSeconds` is 0, else queues it; a structural bench
always queues. The quantity asked is first capped at what the inventory
could hold of the primary output (45 slots × its max stack ÷ the output
quantity). A crafting bench (not the pocket window) refuses a recipe while
the queue's first job is a different one. At most 32 jobs.

Each tick the player's queue runs with the tick's measured length: when
the first job has no unit in progress, a unit starts and its inputs are
taken (from the inventory, or for a structural bench from its input slot;
not in creative); missing inputs drop the job with the missing-ingredient
notification. The running unit's time grows; the window data gets
`progress` (0–1), and an `UpdateWindow` goes out when it changed by more
than 5 %, half a second passed, it reached 1, went back, or the first
time. A finished unit gives one unit's outputs (what does not fit drops),
the leftover time runs the next unit in the same tick, and `queueSize`
(units left in the queue) is updated; when the queue empties the bench
block goes back to its `default` state.

When a bench craft is accepted the bench block switches to its interaction
state `CraftCompleted` (queued) or `CraftCompletedInstant`, and the bench's
`CompletedSoundEventId` plays at the block (SFX). A state switch changes
the block without particles. The extra resources are sent again with the
next window update.

Windows go out once per tick when changed (`UpdateWindow`: id, data, the
window's own slots for a structural window, the extra resources when they
were refreshed), after the inventory.

### Structural benches

`StructuralCrafting` benches (builder's bench) open a `StructuralCrafting`
window whose data has, after the common bench keys, `selected` (0),
`allowBlockGroupCycling`, `alwaysShowInventoryHints`, then the open keys,
`memoriesPerLevel` and `inventoryHints` (indexes over hotbar then storage of
the stacks some bench recipe takes as first input, refreshed when those
change). The window holds 65 slots: an input slot (0) that only takes
items some option can be made from, and 64 option slots that take and give
nothing. Putting an input lists the recipes of this bench with exactly
that one input: header categories first, then by the bench's category
order, then by id; each fills an option slot with its first output;
`dividerIndex` (how many are in header categories) and `optionSlotRecipes`
join the data. `SelectSlotAction{slot}` sets `selected`. A craft queues
the selected option's recipe paid from the input slot and plays the
primary output item's `Drop` sound (UI). `ChangeBlockAction{down}` (when
cycling is allowed) turns the input into the previous or next block of its
block group, keeping the quantity, with that item's `Drop` sound.

### Closing a bench

The client's `CloseWindow` is echoed; the bench block goes back to its
`default` state; the queue is cancelled (the running unit's inputs come
back) and, if there was one, the bench's `FailedSoundEventId` plays; the
close sound plays; a structural input goes back to the inventory (or
drops).

Nothing stays in a crafting bench between uses: its window is rebuilt
each time it opens, the queue is cancelled on closing and a structural
input goes back to the player. The only state a crafting bench block
keeps is its tier and the items spent on upgrading it (tier upgrades are
not done yet; processing benches keep their contents, spec 20).

Diagram benches, tier upgrades and chests near benches are not done yet.

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

## Known recipes

A recipe with `KnowledgeRequired` can only be crafted by a player who
has learned it. What a player learned is a set of item ids (the id of the
recipe's primary output, not the recipe's own id), kept with the player
and saved with it. Learning needs no recipe to exist: any item id can be
learned, and one that makes nothing simply never shows up for the client.

### How a recipe is learned

- `/recipe learn ITEM` (an item, by its key) and `/recipe learn ITEM
  PLAYER` for someone else; `/recipe forget ITEM [PLAYER]` and `/recipe
  list [PLAYER]` do the opposite and print the set. All three are
  sub-commands of `recipe` (spec 28).
- An item whose root interaction has a `LearnRecipe` step (the recipe
  pages and scrolls; most name the item to learn, the plain page reads it
  from the held stack's metadata). The step learns the item and sends the
  same two replies as the command; when it was known already the step
  fails. The chain's next step then takes the held item away.

Replies (chat lines, spec 29): learning answers
`server.modules.learnrecipe.success` (parameter `name`, a message: the
item's name key; green `#00ff00` when it comes from the command, none
from the interaction), or `server.modules.learnrecipe.alreadyKnown`
(`name`; red from the command). Forgetting answers
`server.commands.recipe.forgotten` or `server.commands.recipe.alreadyNotKnown`
(`id`, the item key as text; green and red). Listing answers
`server.commands.recipe.knownRecipes` (`list`, the set printed as
`[a, b, c]`). The forms naming another player use the `….other` keys
(`server.commands.recipe.learn.success.other`, `learn.alreadyKnown.other`,
`forgotten.other`, `alreadyNotKnown.other`, `knownRecipes.other`) with a
`username` parameter first. The player who changed a set gets the
known-recipes packet first, then the reply.

### The known-recipes packet

`UpdateKnownRecipes` holds a map from item id to a crafting recipe. It is
sent once when a player joins (right after the HUD components, with the
set the player saved) and again, whole, after every learn or forget that
changed something. For each learned item that exists, the entry is the
recipe that makes the item, with its `id` set to the item's id (not the
recipe's own); an item no bench makes has no entry. The entries come in
the order of a hash map (an open addressing table: the entry's slot is
the mixed Java hash of the item id, the table holds 32 slots and grows
at 24 entries; the map is read from its last slot down).

Which recipe makes an item is not always one: salvaging benches list many
recipes that give the same ingredient. The server keeps, for each bench
id, a registry of the recipes naming that bench, by category (or none),
and for each item the registry's set of recipes that output it. To pick
the entry it goes through every registry (in the order of the registries'
own table) and every recipe listed for the item (in the order of that
set's table) and the last one found wins. All those sets and maps are
open addressing tables of 32 slots by default, so the order depends only
on the hashes of the ids, except when two ids fall on the same slot, where
the order they were added in decides.

The same sets give the order of `craftableRecipes` in a bench window's
categories.

### The order of the learned set

`/recipe list` prints the set in the order the official server's set
keeps. A set grown by learning is a copy into a new Java hash set (its
table sized for the copy: at least 16 slots, doubled when the new size
passes 3/4; buckets by the hash with its high half folded in; inside a
bucket the older first), so each learn rebuilds the order from the one
before. Forgetting copies into an open addressing set made for that many
entries, removes the id (moving later entries of its run back) and keeps
that set's order, read from the last slot down. A set read back from the
save has no meaningful order on the official server (it is an immutable
set that iterates in an order that changes from run to run), so only the
first list after joining can differ.

### What is saved

The set is part of the player's save: written when the player is saved
(leaving, and periodically) and read when it joins, before the
known-recipes packet goes out. A rejoin gets the same set.

### Crafting with knowledge

A recipe with `KnowledgeRequired` is only crafted when the set holds its
primary output's item id; otherwise the request is dropped without any
reply, as for a recipe the bench does not make. Bench windows list such
recipes whether or not they are known.

## In Aerie (known recipes)

`crafting::Known` holds the set in the official order (a list rebuilt on
each learn or forget by the rules above, kept in the saved text, one item
id per line, in `<world>/recipes/<uuid>.txt`, written on every change).
`Book::known_recipes` builds the packet. The registries (recipes by bench
id, category and output item) are built once from the recipe registry,
with the recipes added in the order of their ids. `/recipe` is in the
command tree (spec 28); the replies are built in `crafting::recipe_reply`.
The join sequence takes the packet from the hub (`known_recipes_payload`)
instead of an empty one. `Book::allowed` takes the player's set.

## Not done yet (known recipes)

- The `LearnRecipe` interaction (see the status below).
- Which recipe wins when several of the same registry make an item
  depends on the order the official server loaded the recipes in, which
  Aerie takes as the order of their ids; two ids on the same slot can come
  out swapped (not seen in the items compared below).
- A learned set read back after a restart lists in the saved order, where
  the official server lists it in a different order every run.
- Permissions: every player may run every `/recipe` form.

## Checked against the official server (known recipes)

Probe scripts on both servers, same item lists, decoded and diffed:
`/recipe learn`, `forget` and `list` for 71 items in a row (every one of
the 55 items that several recipes make, plus 16 others), all the chat
lines and all the known-recipes packets (145 packets, the last with 71
entries) are the same; so is the list order after each step. Crafting
`Food_Pie_Apple` at a cooking bench (needs knowledge) does nothing before
`/recipe learn` and queues after it, with the same inventory.
