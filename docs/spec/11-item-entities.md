# 11 — Dropped items

Behaviour of the official server, written from reading it and checked
with captures; no code. Items on the ground: how they spawn, fall, land,
merge, get picked up and disappear, and what clients receive.

## Spawning

A dropped stack becomes an item entity with: the stack; a merge delay of
1.5 s; a pickup delay of 0.5 s; a pickup throttle of 0; a lifetime (the
item's `ItemEntity.Lifetime`, else the gameplay config's
`ItemEntity.Lifetime`, else 120 s; the default gameplay config says 600);
physics values mass 1, drag coefficient 0.5; a collision box 0.5 wide,
0.5 deep and 0.5 high, horizontally centred on the position with its
bottom at it; and a velocity:

- one stack: (0, 3.25, 0);
- several stacks from one event: spread on a circle of radius 3 (m/s)
  starting at a random angle, (x, 3.25, z) each.

Block drops spawn at the block's bottom centre (x + 0.5, y, z + 0.5).

A player throwing a stack (`DropItemStack`, the slot's stack taken): the
item starts at the player's position raised by the eye height plus the
look direction (pitch, yaw: x = −sin(yaw)·cos(pitch), y = sin(pitch),
z = −cos(yaw)·cos(pitch), with the official 4096-entry sine table), with
that direction × 6 as velocity, a pickup delay of 1.5 s, and the player
hears `SFX_Player_Drop_Item` (UI). Creative drops (`DropCreativeItem`) and
stacks that do not fit after crafting are thrown the same way at speed 1,
without the sound; out of creative a creative drop only gets the
`server.general.setCreativeItem.notInCreativeMode` notification. Checked
with a throw on both servers: same start (eyes 1.6 above the feet), speed
and fall.

## Every tick

The world ticks 30 times a second, but `dt` is the measured length of the
tick (the official server sends it in `NetworkTick.tick_duration_micros`
every tick, with the tick number), so positions depend on timing.

In this order, for each item:

1. Out of blocks: if the block at the item's position is solid and one of
   its hitbox boxes overlaps the item's box, the item moves to the nearest
   of the four horizontal neighbours that is not solid (to its bottom
   centre), or, if none, gets an upward speed of 7 × the block's height.
2. Gravity: area = width × depth of the box (0.25); terminal speed
   v_t = √(64 · 1000 · mass / (0.001225 · area · 10⁶ · drag)) downward
   (about 20.44 blocks/s); acceleration a = 32 · (sign(v_t) − sign(v) ·
   |v / v_t|³); the vertical speed moves by a · dt towards v_t without
   passing it.
3. Movement: the step is velocity · dt. The item's box is swept along it
   through the solid blocks' hitbox boxes (block positions with their
   hitbox, air and non-solid blocks let it through). On the first hit: if
   the hit face points up, the item stops (velocity zero) at the hit
   point; otherwise the velocity loses its part along the face's normal
   and the item does not move this tick. Without a hit it moves by the
   step. Below y = −32 the item is removed.
   The sweep, per axis, compares the moving box with each block box: a
   gap ahead closer than the step gives an entry time (gap / step,
   clamped to [0, 1], 0 under 10⁻⁸) with the face normal; already
   overlapping on an axis gives no entry on it; the latest entry over the
   three axes is the hit if it is before every exit. Distances within
   10⁻⁵ count as touching.
4. Merging (stackable items whose stack is below the item's max stack):
   the merge delay counts down; at 0 it restarts at 1.5 s and the item
   takes, nearest first within 2 blocks (the item's `MergeRadius`
   otherwise), every other item of the same stack kind: whole stacks
   are removed and added to it; a stack that does not fit leaves the
   remainder; it stops once full. Both lifetimes restart.
5. Pickup: the pickup delay counts down; once at 0, the throttle counts
   down and every 0.25 s the players within 1.75 blocks (the item's
   `PickupRadius` otherwise) of the item, nearest first, get it. The stack
   goes into the player's inventory as the player's preferences say
   (default hotbar first: existing stacks of the item over hotbar,
   storage, backpack in slot order, then empty slots in that order, each
   slot up to the item's max stack). All taken: the item is removed;
   partly: the item keeps the rest, its lifetime restarts. For what is
   taken the player gets a notification (`server.general.pickedUpItem`
   with the item, when the gameplay config shows pickup notifications —
   on by default) and the sound `SFX_Player_Pickup_Item` at the item, and
   a short-lived copy of the item flies from the item to the player's
   position raised by a fifth of the eye height over 0.15 s (linear),
   then disappears.
6. Despawn: past its lifetime the item is removed.

## What clients receive

Per tick, in `EntityUpdates`:

- a new item (sent the tick after it spawned, already one physics step
  on): `ItemUpdate` (the stack, entity scale 0), `IntangibleUpdate`,
  `TransformUpdate` (position only); the order of entities inside one
  `EntityUpdates` follows the official server's entity storage and is not
  compared;
- an item that moved: `TransformUpdate` with its position; nothing while
  it rests;
- an item whose stack changed (merge, partial pickup): `ItemUpdate`;
- removed items in the `removed` list.

## Checked against the official server

The tree drops of spec 10: the four trunks spawn one tick apart, rise and
fall as above (positions 141.0726, 141.1097, 141.1111, …; replaying the
captured tick lengths through these rules gives every position within
10⁻⁴), land on
the ground at a whole block height, send nothing while resting, and 45
ticks after the first spawned the first item takes the other three (three
removals and one `ItemUpdate`).

`tools/diff/tree-pickup.txt` (a floor under the tree, then the probe walks
to the drops) and `tools/diff/items.py`, on both servers: same spawn ticks
and packets, same rise, fall and landing, merge with three removals (newest
first) and one `ItemUpdate`, pickup with the same `Notification` (same
bytes), sound (`PlaySoundEvent3D` 419, UI, at the item) and inventory in
that order, and the flying copy (five steps of the same size, removed the
tick after it arrives). Left over: one-tick shifts from the measured tick
lengths, and network ids (Aerie numbers items from 1,000,000).
