# 12 — Breaking blocks

Behaviour of the official server, written from reading it; no code. A
player's hit on a block: the spec it uses, damage and health, effects,
breaking and drops.

## The held item

The server uses the item it holds in the player's active hotbar slot (the
slot the chain names), not the item id the client sends.

## Which spec

From the block's `Gathering.Breaking` (`GatherType`, `Quality`):

- a weapon or a builder tool: no spec;
- an item without `Tool` (or no item): the bare hand's spec for the gather
  type (`Item/Unarmed/Gathering/<GatherType>`), none if its quality is
  below the block's;
- a tool: its first `Specs` entry of that gather type (incorrect ones
  included), none if its quality is below the block's, none if it has no
  such entry.

## One hit

- A spec with power: damage = power (for a broken tool, × (1 − the
  gameplay config's `ItemDurability.BrokenPenalties.Tool`, 0.75)); breaking
  it drops `Breaking`
  (`ItemId`, `DropList`, `Quantity`).
- No spec, and the block is not `Soft`: a held item that is no weapon
  shows the unbreakable effect (the gameplay config's
  `Gathering.UnbreakableBlock`: its particle system at the block centre for
  players within 75 blocks, its sound) and the tool's
  `IncorrectMaterialSoundLayer`; nothing else happens.
- No spec, `Soft` block: a weapon does nothing unless `IsWeaponBreakable`
  (true by default); otherwise damage 1, drops `Soft` (`ItemId`,
  `DropList`).
- A block with `UseDefaultDropWhenPlaced` placed as decoration drops its
  default item instead.

Health starts at 1 and loses the damage; at 0 (within 10⁻⁵) or below the
block breaks. Otherwise the block sound set's `Hit` sound plays at the
block centre. Either way the spec's `HitSoundLayer` (else the tool's) plays
at the centre, and an incorrect spec shows the gameplay config's
`Gathering.IncorrectTool` effect.

## Breaking

The block sound set's `Break` sound at the block centre, the block becomes
air with a `Break` particle, its area is marked for support checks (spec
10), and the drops spawn at the block's bottom centre (spec 11): with a
drop list, rolled `Quantity` times; an item, `Quantity` of it; neither,
the block's own item. In creative a hit breaks the block at once, without
drops.

## Health over time and what clients see

A damaged block left alone for 5 s heals 0.1 per second (with the tick's
length) until whole. Health changes are summed per block and sent at the
end of the tick, per section: one block `UpdateBlockDamage` (position,
health, change), more `UpdateBlockDamages` (section, then index, health,
change per block, in the order of the official server's map). The hit that
breaks a block sends no damage. A client loading a column gets
`UpdateBlockDamages` (change 0) for every damaged block in it.

## Tool wear

Every stack carries a durability and a max: a stack made from an item id
(given, crafted, dropped by a block, a creative pick) starts at the item's
`MaxDurability` for both; a max of 0 never wears; a stack is broken at
durability 0. Stacks only stack (inventory, merging on the ground) with
the same item, durability and max. Dropping a stack keeps its durability.

After a hit that does damage (not the unbreakable or nothing cases), out of
creative, the held stack wears unless it never does: the loss is 0 on a
soft block, on a block without gathering, or for an item without `Tool`;
otherwise the first `DurabilityLossBlockTypes` entry that names the block
or a block set holding it (sets as sent in `UpdateBlockSets`) gives the
loss, else the item's `DurabilityLossOnHit`. Durability goes down by the
loss, kept within 0 and the max, and the slot is replaced, which sends the
hotbar even when nothing changed. A stack that breaks with this hit sends
its player the chat line `server.general.repair.itemBroken` (param
`itemName`: the item's name, colour `#ff5555`) and the 2D UI sound
`SFX_Item_Break`.

## Break tools

The `BreakBlock` interaction a hit runs can name a tool type (`Tool`) and
require it (`MatchTool`). Which one runs comes from the held item's
interactions for the chain's type, with the item's `InteractionVars` in
place of the defaults their `Replace` interactions name (in the game:
pickaxes and drills `Pickaxe`, shears `Shears`, everything else none; the
bark scraper's `Scraper` attack is not given to any item). A tool that must
match does nothing on a block whose `Gathering.Tools` has no entry for it.

When the block's health runs out and the block has an entry for the tool:
if the entry names a state that is another block, the block becomes that
state (rotation 0, no support area marked), the old block's break sound
plays and the entry's drops fall (its item and drop list once; neither:
the block's own item); otherwise the block breaks without its usual drops
and the entry's drops fall. In the game: a pickaxe on gravel drops its
`Soil_Gravel_Pickaxe` list, shears give leaves, webs and the like back as
themselves.

## When the world forbids it

The world's gameplay configuration has flags for breaking and for
gathering (picking up by using). A break operation on a block of a world
that forbids breaking fails its chain at once: no damage, no wear, no
effects. The same for a pick-up when gathering is forbidden. The
configuration can name a tag whose blocks may still be broken; Aerie does
not know block tags yet, so it treats every block as not having it.

An environment that forbids changing blocks stops damage from being
applied (the block is resent) in any game mode, but no shipped environment
forbids changes and Aerie's chunks share one environment, so it has no
effect yet.

In Aerie the flags come from the same server table as spec 15's
(`W` lines); the chain host refuses before the hit runs.

## Not done yet

The bypass tag of the world's breaking flag; the environment rule (above);
the pick-up refusal was written from the reference but not compared, for
lack of a recording.

Wear from weapons and armour. A hit lands when its chain
starts; the official server lands it when the chain reaches its break
operation, so with fast clicking a late chain can land after the next
one's first hits.

## Break shapes

A tool can name a *break shape*: a box or a cylinder, `width` across,
`height` up and `depth` into the view (each 1 to 32, at most 1024
cells), centred or not, moved by an offset (rounded, in the shape's own
axes), turned by the look or by the face hit, and a wear mode (once per
swing, the default, or once per block).

The shape's axes: *View* takes the main axis of the look direction as
"in" (ties go to y, then x); looking mostly up or down, "up" is the
main horizontal axis of the look (away from it when looking down), and
"up" is the world's up otherwise. *Surface* takes as "in" the axis
whose face of the aimed block the look ray enters last, the same way
for "up". "Across" is up × in.

Its cells, in the order they are hit: for each depth step, for each
column across, each row up — from the low bound (−(size − 1) / 2 when
centred, else 0). A cylinder keeps the cells whose distance from the
middle, scaled by half the width and half the height, is at most 1.
Each cell is the aimed block plus the offset plus the cell, along the
axes; a cell is hit once even when two map to it, and cells outside
the world's height are skipped.

In creative every cell breaks. Otherwise each cell takes the hit as
any single block (damage, drops, sounds); with per-block wear the tool
wears for each cell it damaged (the held stack read again after each),
with per-swing wear it wears once after all of them, by the block that
was at the aimed cell before the swing.

Only debug pickaxes ship with shapes: a 3 × 3 box worn per swing and a
5 × 5 cylinder worn per block.

## Checked against the official server

Stacks given and thrown carry the same durability and max on both
servers (a crude hatchet: 200 of 200, in the inventory and on the ground).
A recorded client's 27 hatchet hits on stone, replayed on both servers
(root interaction ids translated by name, the hatchet in the active slot):
the same damage per hit, health, hit sounds, wear (0.25 a hit, stone being
in the hatchet's sets), healing after 5 s and damage packets, but for the
one late chain above. (The official server had cancelled earlier replays
because the two servers number root interactions differently.)

Break shapes: two recorded swings of each debug pickaxe replayed on a
5 × 5 layer of stone, looking down at an angle (`probe --look`): the
3 × 3 box and the 21-cell cylinder break the same cells on both, with
the same hit and break sounds in the same order, and the tool wears the
same (0.25 a swing for the box, 0.25 a block for the cylinder). The
probe confirms the server's teleport as a client does: until then the
official server ignores its look.

With the world set to a configuration that forbids breaking (three recorded
hatchet swings on a log, adventure): the same chain answers, no block change,
no sounds and no wear on both servers. Compared without the rule, those
swings differ on the first fork's last steps between the servers (the
official one runs the "break failed" root); that is outside this rule.

## Picking a block up by using it

A block whose gathering has a harvest entry is picked up by the harvest
form of the break interaction (the empty hand's use, when the block has
no use of its own and no entity is targeted), when gathering is allowed:

1. The block goes (its area is checked for support).
2. Its harvest drops — the entry's item or drop list, else the block's own
   item, once — go straight into the player's inventory by the pickup
   preferences (spec 14); what does not fit is thrown as the player
   throws an item (spec 11).
3. For what was taken the player gets the pickup notification, the
   pickup sound (not positioned) and the item flying to them from the
   block's centre.
4. The block's harvest sound plays at the block's centre.

A block without a harvest entry fails the step.

Checked against the official server (a rubble block used with an empty
hand, a made-up client chain): the same chain states, block change,
inventory, notification and both sounds.
