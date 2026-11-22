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

## Not done yet

Break shapes (tools that break several blocks, with per-block or
per-swing wear); wear from weapons and armour. A hit lands when its chain
starts; the official server lands it when the chain reaches its break
operation, so with fast clicking a late chain can land after the next
one's first hits.

## Checked against the official server

Stacks given and thrown carry the same durability and max on both
servers (a crude hatchet: 200 of 200, in the inventory and on the ground).
A recorded client's 27 hatchet hits on stone, replayed on both servers
(root interaction ids translated by name, the hatchet in the active slot):
the same damage per hit, health, hit sounds, wear (0.25 a hit, stone being
in the hatchet's sets), healing after 5 s and damage packets, but for the
one late chain above. (The official server had cancelled earlier replays
because the two servers number root interactions differently.)
