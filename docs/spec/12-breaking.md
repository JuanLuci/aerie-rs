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

- A spec with power: damage = power (× 0.25 for a broken tool: the
  gameplay config's tool penalty 0.75); breaking it drops `Breaking`
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

## Not done yet

Tool wear (`MaxDurability`, `DurabilityLossOnHit`,
`DurabilityLossBlockTypes`) needs durability in the inventory; tool states
(`Gathering.Tools`, such as stripping a trunk); break shapes (tools that
break several blocks).

## Checked against the official server

Not yet: replaying a recorded client's hits makes the official server
cancel the chains ("client failed to send client data"), because the
recording kept only some of them. Recordings now keep every chain with its
time, and the probe replays them at the recorded pace.
