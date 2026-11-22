# 15 — Placing blocks

Behaviour of the official server, written from reading it and checked
with captures; no code.

## What the client reports

A placement is an operation of an interaction chain whose client data has
a block position, a block rotation and a placed block id. The server acts
on each operation once (the client repeats chain data in later updates).
The held stack is the one in the chain's active hotbar slot.

## Checks

- No held stack: nothing happens.
- Reach: the block's centre is within the held item's use distance plus 2
  of the player's eyes (`InteractionConfig.UseDistance`: 5 in adventure,
  6 in creative by default, 1 for a mode an item's map leaves out; in
  creative at least the player's creative interaction distance, 10 by
  default, at most 128). Too far: nothing happens.
- The held item must place a block. The block placed is the client's if it
  is that block or one of its placement overrides (wall, floor, ceiling);
  otherwise the item's own block. An unknown client block id fails.
- In adventure the held slot loses one item first; if the slot no longer
  holds that stack the player gets `server.modules.interaction.failedCheck`
  and nothing is placed. A missing, empty or unknown block then gets
  `failedCheckBlock`, `failedCheckEmpty` or `failedCheckUnknown` (the item
  stays used up).
- Every cell the block fills (spec 10, fillers) must be air, a non-solid
  block, or an unknown filler (not checked with creative no-physics).

## Placing

Each non-solid block in those cells breaks first: its break sound, its
cell emptied with the physics particle, its area marked for support
checks, and its soft drop (or its own item) dropped. Then the block is set
with its rotation (fillers, particles, support marking as spec 10). A block
that can be placed as decoration (ignores support when placed, or drops
its default item when placed) is marked decoration, as is everything in
creative no-physics.

A placement that fails at the cells, or sets nothing new, gives the item
back (to its slot, else anywhere in the hotbar; if neither,
`failedAddBackAfterFailedPlace`) and sends the client the block the
position really holds.

## What clients receive

The block change goes out with the tick's changes; the placement's
particles go out after the block changes and before the height changes;
the hotbar after that.

## Not done yet

The server's own chain updates back to the client (the official server
answers each chain with its state); the gameplay config's
`AllowBlockPlacement`; environments that forbid changes (Aerie's world has
one environment, which allows them); prefab lists; connected blocks
(stairs, fences); a block entity carried in the item's metadata.

## Checked against the official server

A recorded placement chain moved to the same kind of spot on both servers
(its root interaction ids translated by name: the two servers number root
interactions differently): the same block, particle, height and hotbar,
in the same order; onto a solid block, the same resent block and hotbar
with the item given back.
