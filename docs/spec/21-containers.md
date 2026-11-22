# 21. Blocks that hold items

A chest is a block with an item container as its block state (spec 20):
a number of slots given by the block (20 unless it says otherwise).

## Opening

The block's use interaction opens it (a block type of spec 18). For a
player without a window open on that block already:

1. The player's page becomes the bench page and a container window opens
   with the block's item id as its data and the container's slots as its
   inventory section.
2. The sound of the block's "open window" state plays at the block's
   centre.
3. When this is the first window on the block, the block switches to its
   "open window" interaction state (the lid opens for everyone).

Several players can have the same container open; each sees the others'
changes (the window is updated in the tick of the change).

## Moving items

Every slot takes and gives any item; the inventory moves of spec 14 work
between the player's sections and the window's.

## Closing

When a window closes: with no window left on the block, the block
switches to its "close window" state — unless the block is set to stay
open while empty and is empty — and the close sound plays at the block.

## The block goes

When the block is replaced by anything but one of its own interaction
states, its windows close and everything it held drops at the middle of
its base (x + 0.5, y, z + 0.5). A container block set over another
container takes its items instead.

## In Aerie

Containers live in the world state per position, written with the block
state file of spec 20 (`@ x y z container <slots>`, a `slot` line per
filled slot); empty ones are not written. Block data comes from the
asset side: capacity, stay-open flag and the two state sounds.

## Not done yet

Sorting (the window's sort action); double chests (two blocks joined by
connected-block rules sharing a larger container); loot lists filled on
first opening; a container replaced by another one keeping its items; a
container carried as an item; windows of a player who leaves; the slots
over a block's new, smaller capacity dropping.

## Checked against the official server

A small chest placed, opened with a recorded use chain, two stones moved
in, closed (`chest` scenario with `tools/diff/chains.py` and the window
packets): the same page, window, sounds, block states, inventory and
window updates, and chain states.
