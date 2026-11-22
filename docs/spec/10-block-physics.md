# 10 — Block physics (support)

Behaviour of the official server, written from reading it and checked
with captures; no code. Covers which blocks need support, when support is
checked, what a block without support does, and what clients receive.

## Data

Most of it is in the `BlockType` registry entry the client already gets
(spec 07): `support`, `supporting`, `max_support_distance`,
`block_supports_required_for`, `material`, `opacity`, `hitbox`,
`ignore_support_when_placed`, `tag_indexes`. The server also uses, from
the block's asset: `SupportDropType` (`Break` default, `Destroy`, `Fall`),
`Gathering` (`Physics`, `Breaking`, `Soft`, `Harvest` drops and
`UseDefaultDropWhenPlaced`), and drop lists (`Item/DropLists`).

Defaults applied when the asset is read:

- A block with no `Support` whose material is `Empty` and whose group is
  not `@Tech` requires a `Full` face below it (plants, grass).
- A block with no `Supporting` that is drawn as `Cube`, `CubeWithModel` or
  `GizmoCube` with material `Solid` offers a `Full` face on all 26 faces.
  Any other block without `Supporting` offers none.
- `MaxSupportDistance` 0; `BlockSupportsRequiredFor` `All`.
- A requirement (`Support` entry): `FaceType`, `SelfFaceType`,
  `BlockSetId`, `BlockTypeId`, `FluidId`, `TagId`, `Filler`, `MatchSelf`
  (`Ignored` default), `Support` (`Required` default, `Ignored`,
  `Disallowed`), `AllowSupportPropagation` (true), `Rotate` (true).
- A supporting face: `FaceType` (`Full` default), `Filler`.

A block "has support rules" when it has a non-empty `Support` or a
`MaxSupportDistance` above 0. Others are never checked.

## Faces and neighbours

There are 26 faces: the 6 sides, the 12 edges (`UpNorth` …) and the 8
corners. Directions: up +Y, down −Y, north −Z, south +Z, east +X, west −X;
an edge or corner is the sum of its sides. In assets, face names also come
merged: `All` (26), `BlockSides` (6 sides), `CardinalDirections` (N, E, S,
W), `Horizontal` (N, NE, E, SE, S, SW, W, NW), `UpCardinalDirections`,
`DownCardinalDirections`; a merged key adds its requirements to each face.

For a face, the neighbour faces it meets are:

- a side: the opposite side;
- an edge: its direction turned by 90°, 180° and 270° — about Y (yaw)
  for the up/down–north/south edges, about X (pitch) for the four
  horizontal edges, about Z (roll) for the up/down–east/west edges (the
  official server names these the other way round; the turns listed are
  what it does);
- a corner: its direction turned by yaw 90°, yaw 180°, pitch 90°, pitch
  180°, pitch 270°, roll 180°, and the opposite corner.

For each such neighbour face the neighbour position is the block offset by
the face direction's components that are opposite to the neighbour
face's direction (a component is kept when the two directions have
opposite signs on that axis). Offsets landing inside the block's own
hitbox (its own cell for a 1×1×1 block) are skipped.

A rotated block (rotation index = roll·16 + pitch·4 + yaw, quarter turns)
uses its requirements and supporting faces rotated: a direction is turned
about Z by the roll, then X by the pitch, then Y by the yaw (Y by 90°:
(x, y, z) → (z, y, −x); X by 90°: (x, −z, y); Z by 90°: (−y, x, z)). A
requirement with `Rotate` false keeps its face.

## Checking one block

Done only for the main cell (filler 0) of a block with support rules, and
never for a block marked as decoration (support value 15). For each face
with requirements, for each neighbour of that face, for each requirement
(that applies to the cell's filler offset), the neighbour satisfies the
requirement when all of these hold:

- `BlockSetId`: the neighbour is in the set;
- `BlockTypeId`: the neighbour is that block;
- `FluidId`: the neighbour is material `Empty` and holds that fluid;
- `TagId`: the neighbour has the tag;
- `FaceType`: the neighbour offers a supporting face of that type on the
  neighbour face (for its rotation and filler);
- `SelfFaceType`: this block offers that face type on the face (looked up
  with the neighbour's rotation, as the official server does);
- then `MatchSelf`: `Required` also needs the neighbour to be the same
  block; `Disallowed` needs it not to be.

A requirement satisfied with `AllowSupportPropagation`, when the block's
`MaxSupportDistance` is above 0, offers the neighbour's support value as a
distance (15, decoration, counts as 0, so it gives 1). Then by `Support`:
`Required` satisfied → the face is supported; `Disallowed` satisfied →
the face fails; `Ignored` only offers its distance.

Results, in order: any neighbour face supported and not failed → the
block is directly supported. No `Required`/`Disallowed` requirement tested
at all → no rule applies. Otherwise, with a smallest offered distance d,
the block is supported at distance d + 1 if that is at most
`MaxSupportDistance`; else it is unsupported.

A block whose hitbox extends past its cell checks every cell of the
hitbox (rounded outward) and combines them by `BlockSupportsRequiredFor`:
`Any` — one cell directly supported wins, an unsupported cell makes it
unsupported unless another wins, else the largest distance; `All` — an
unsupported cell loses at once, else directly supported if any cell is,
else the largest distance.

## Support values and what follows

Each block keeps a support value 0–15 (0 when never set, 15 for
decoration). After checking a block:

- unsupported: the block is removed (below);
- no rule: nothing;
- directly supported: value set to 0; if it was not 0, its 3×3×3
  neighbourhood is marked for checking;
- supported at distance d: if the value already was d, the neighbourhood
  is marked only for neighbours whose value is at most d − 1; else the
  value becomes d and the whole neighbourhood is marked.

Placing a block with support rules resets its value to 0; placing one
without rules clears it.

## When blocks are checked

The world ticks 30 times a second. Blocks are "marked" for checking; each
tick, every section takes the blocks marked before the tick started and
checks them in index order (`y·1024 + z·32 + x` within the section); blocks
marked during the tick are checked the next tick. So a chain of blocks
without support goes one block per tick.

Marking happens:

- when a player breaks a block (unless the player has physics off): the
  block's hitbox grown by one block on every side (3×3×3 for a plain
  block);
- when a player places a block (same area);
- when a block is removed by physics (same area);
- by the official `/block setticking x y z` (one block).

`/block set` changes a block without marking anything.

A block placed by a player is marked as decoration (value 15, never
checked) when its asset has `IgnoreSupportWhenPlaced` or
`Gathering.UseDefaultDropWhenPlaced` (trunks, for instance), unless the
player overrides placement restrictions or has physics off.

## Removing an unsupported block

By `SupportDropType`:

- `Break`: removed with drops, a break sound and a physics particle;
- `Destroy`: removed with the sound and particle, no drops;
- `Fall`: becomes a falling block entity (spec 11).

Drops: `Gathering.Physics` (item and/or drop list), else `Breaking`
(with its quantity), else `Soft`, else `Harvest`; with neither item nor
list, the block's own item. Each drop is an item entity at the block's
bottom centre (x + 0.5, y, z + 0.5), spawned as spec 11 describes. The
break sound is the block sound set's `Break` event, played at the block's
centre to players within the sound's maximum distance.

Drop lists: a container of type `Multiple` (`MinCount`, `MaxCount`,
`Containers`) rolls round(r·(max − min) + min) times and, each time, takes
every child whose `Weight` (100 by default) is at least r·100 (a new r per
child); `Choice` (`RollsMin`, `RollsMax`, `Containers`) rolls
min + ⌊r·(max − min + 1)⌋ times a weighted pick (a running sum of weights
compared with min(r, 0.99999)·total); `Single` (`Item`: `ItemId`,
`QuantityMin` 1, `QuantityMax` 1, `Metadata`) adds its item; `Droplist`
(`DroplistId`) includes another list once per roll chain; `Empty` adds
nothing. Each item's amount is uniform in [min, max]. The randomness is
unseeded on the official server, so only the rules can be compared.

## Blocks of several cells (fillers)

A block whose hitbox (rotated) reaches past its cell fills every cell of
the hitbox rounded outward: the main cell has filler 0, the others the
same block and rotation with their offset from the main cell packed as
`x & 31 | (z & 31) << 5 | (y & 31) << 10` (5-bit signed parts). Setting a
block removes the other cells of the block that was there (those still
holding it) and fills the new block's other cells (removing whatever block
each held, with its own other cells); each cell change sends its particle
and goes into the tick's batch like any block. Support checks skip filler
cells; hitting or using a filler cell acts on the main cell. Checked with
`/block set` of a workbench and its removal: the same packets, particles
and batch order as the official server.

## What clients receive

At the end of each tick, per section changed during the tick, to every
player who has the column loaded:

- one change: `ServerSetBlock` (x, y, z, block, filler, rotation);
- 2–1023 changes: `ServerSetBlocks` (section x, y, z and one command per
  block: index, block, filler, rotation). The commands come in the
  iteration order of the official server's changed-position set: an open
  addressing integer hash set (the public fastutil `IntOpenHashSet`: table
  of a power of two, load factor 0.75, hash `h = k·0x9E3779B9; h ^ h>>>16`,
  linear probing, iterated from the last slot down, key 0 first). Each
  section has two such sets that swap every tick and are cleared, never
  shrunk, so the order depends on how big each has grown;
- 1024 or more: the whole section again (`SetChunk`).

Then, per column whose height changed: `ServerSetHeight` (x, z, height)
for one, `ServerSetHeights` (column x, z; commands index z·32 + x and
height, in increasing index) for 2–511, the whole heightmap for more. The
height is the highest block whose opacity is not `Transparent`.

Particles are sent as each block changes: `SpawnBlockParticleSystem` (old
block, event, position at the block centre) — `Break`, or `Physics` for a
removal by physics, when a block becomes air; `Build` when air becomes a
block; `Break` of the old and `Build` of the new otherwise. The official
server queues particles and block changes on different paths, so their
order within a tick varies between runs.

## Checked against the official server

`tools/diff/tree-fall.txt` (commands) and `tools/diff/blocks.py` (side by
side). Aerie matches the official server on every removal, its tick, both
`ServerSetBlocks` batches with their command order, the columns whose
height changes, the physics particles and the four trunk drops. Left over:
height values (Aerie's terrain is not the official generator yet) and the
particle/block order inside a tick (it varies on the official server).

Probe script (both servers): build a stone with four oak trunks on it and
seven oak leaves around the top with `/block set`, remove the stone with
`/block set … Empty`, then `/block setticking` the lowest trunk. Official
result, tick by tick: trunk 141; trunk 142; trunk 143; trunk 144 and the
two leaves after it in index order (one `ServerSetBlocks`); the leaf before
the trunk (it was checked while the trunk still stood) one tick later; the
next leaf two ticks later; the last three leaves together. Four trunk items
drop (the leaves' list gives fibre 2% of the time).
