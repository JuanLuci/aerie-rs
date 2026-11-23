# 23. Connected blocks

Stairs, roofs, fences, walls, doors and chests change their block (and
rotation) to fit the blocks around them. A block type can name a
connected-block rule set; the rule set answers one question: at this
position, with this block and rotation, which block and rotation should
be here?

## When rules run

After a block is placed or removed (also by harvest and by breaking), at
the changed position, with the normal of the face it was placed on:

1. When the placed block has a rule set and sits at its base cell:
   - a rule set in "update, place, update" mode first updates the
     neighbours (below) without touching the placed block, and the block
     and rotation are read again;
   - the rule set is asked for the block as a placement; a different
     answer replaces the block (and rotation);
   - a rule set in "ignore updates" mode stops here.
2. The neighbours are updated.

## Updating neighbours

Breadth first from the changed position, three steps deep. At each
position taken from the queue, the 26 cells around it are looked at (x,
then z, then y, each from −1 to 1): a cell whose block has a rule set
that is not "only on placement" is asked for its block as an update — a
part of a multi-cell block is asked at its base cell with the part's
rotation — and an answer that differs from what is there is collected.

Every collected position not seen before in this run (and not the queue
position itself) gets its new block, then the extra blocks the answer
carries (offsets from that position), and — while fewer than three steps
from the start — is queued itself. When the neighbours are updated ahead
of a placement, the start position is not changed.

Blocks set by these rules do not run block physics for their area and do
not trigger rules again by themselves.

## Stairs

A stair rule set names five blocks — straight, corner left and right,
inverted corner left and right (an absent one is the straight block) —
and a material name. Only stairs of the same material connect. A stair's
facing is its yaw, turned half round when it is upside down (a pitch
other than none).

For the stair at a cell:

1. **In front** (one cell south, turned by the facing): a stair there,
   the same way up and facing a quarter turn away, makes this one an
   inverted corner — left when the other faces a quarter turn further,
   right when a quarter turn back — unless the other's own shape already
   closes that corner.
2. **Behind** (one cell north, turned by the facing): the same test makes
   it a corner, and wins over the inverted corner — but not when the
   cell in front of that other stair holds a stair with exactly this
   stair's rotation (the two run on straight).
3. Upside down, left and right are swapped.

The answer is that shape's block with the stair's own rotation.

## Roofs

A roof rule set has the stair shapes twice — regular and, optionally,
hollow — an optional top piece, a width (1 or more: a wide roof is one
block over several cells) and a material name; it connects like a stair
of that material, with these differences:

- A corner (in front or behind) only counts when the roof's width is
  fulfilled: the cells beside it toward the corner, up to the width, hold
  the same block with the same rotation (as their own base cell or as
  this block's part). The stair behind is looked for a width away (for a
  wide roof also one cell further, when that one is not straight).
- A roof still straight looks one level up: a roof behind it there
  turned the same way, and a roof on exactly one side turned a quarter
  toward it (corner shapes count a quarter further), make it an inverted
  corner on that side — a valley. Still straight, it looks one level down
  the other way round (in front, sides swapped, inverted corners instead
  of corners) and becomes a corner — the end of a ridge.
- A straight roof whose four neighbours one level down all slope away
  from it (each turned to its side, no outward corners) becomes the top
  piece.
- Otherwise, when the block under it has no supporting top face, the
  hollow shape is used; else the regular one. A wide corner that changes
  side also puts the new block at the cell on its old side.

## Templates

Walls and fences, chests, doors, village walls, bookshelves and rails
share *templates*. A block names a template and gives, for each of the
template's shapes, the block that is that shape (a block pattern: a
block, with an optional rotation). The template has:

- its shapes, each with *face tags* (names on the faces of the shape:
  where it connects) and a list of *patterns*;
- a default shape;
- whether blocks of other materials count as neighbours (when not, only
  blocks that share the very same shape map do);
- whether its blocks are only asked when placed.

The shapes are tried in the order a Java hash map of their names lists
them (not the order of the file), each shape's patterns in file order.
The first pattern that holds gives the answer. When none does, the
answer is the default shape's block with the rotation the block has
(nothing, when the block has no block for that shape).

### A pattern

A pattern is a list of rules, each about the cell at a relative
position. It has: whether its rules turn with the block (they do unless
it says otherwise), an extra yaw for the answer, the transformations it
may be tried in, and whether it is only for placement or only for
updates (it is skipped in the other case).

The transformations, in order: as written; then, when the pattern may
be turned, by one, two and three quarter turns of yaw; then the same
again mirrored in x when it allows that, then mirrored in z.

For one transformation:

1. The total rotation is the transformation's yaw, plus the block's own
   yaw, pitch and roll when the rules turn with the block.
2. Each rule in turn: its position is mirrored (x or z negated), then
   — for a rule with placement normals — the rule holds when the face
   the block was placed on is one of them, and the whole pattern fails
   when it is not. Otherwise the position is turned by the total
   rotation (roll, then pitch, then yaw) and the cell there is read; an
   unloaded cell fails the pattern.
3. The block found is seen *turned back*: its rotation minus the total
   rotation, per axis, and its part offset turned back the same way
   (pitch, yaw, roll). Under a mirror its yaw is then mirrored by its
   block's flip type, the part offset following.
4. The rule is tested (below); the first rule that does not hold ends
   this transformation and the next one is tried.
5. When all hold, the answer is the shape's block with: its pattern
   rotation, plus the pattern's extra yaw, mirrored by the answer
   block's flip type under a mirror, plus the total rotation per axis;
   and for a block whose flip type is orthogonal ending pitched half
   round, a quarter turn of yaw back.

Mirroring a yaw by flip type, across the x or the z axis: *symmetric*
blocks turn half round when their yaw lies along the mirrored axis
(even yaws lie along z, odd ones along x) and stay otherwise;
*orthogonal* blocks step a quarter — forward for odd yaws, back for even
ones, the other way round across z; *orthogonal inverse* blocks step the
opposite way.

### A rule

A rule includes or excludes. It names any of: face tags per direction,
shapes (a shape name with a rotation and a part offset), block types.
Against the block found (turned back as above):

- With face tags: a block that has no template fails an including rule
  and passes an excluding one. Otherwise, for each shape the block
  stands for (the shapes of its own map that list it), if materials may
  mix or the two blocks share their shape map: each of the rule's
  directions is turned back by the found block's yaw, and when that
  shape has one of the rule's tags on that face the rule gives its
  verdict — an including rule holds, an excluding one fails.
- With shapes, likewise: when one of the shapes the block stands for is
  listed with exactly the block's turned-back rotation and part offset,
  the rule gives its verdict.
- With block types: when the block is one of them, the rule gives its
  verdict.
- When nothing gave the verdict, an including rule fails and an
  excluding one holds.

Blocks that are an interaction state of another block use that block's
template, shape map and flip type.

## In Aerie

`connected.rs` answers for a cell through a small view of the world; the
world applies the answers after a placement, a break and a harvest.
Stair and roof rule sets come with the block types' registry packets;
templates, each block's shape map and flip types come from the asset
side's physics table (`CT`, `CB`, `CF` lines).

## Not done yet

- The *patterned* rule sets (three pipe sets, eight blocks) with their
  "update, place, update" and "ignore updates" modes.
- The face a block was placed on is not passed in yet (rules with
  placement normals see none); no shipped template has such a rule.
- A shape whose block pattern lists several blocks takes the first one;
  the official server picks by a random number seeded from the
  position. No shipped pattern can answer with such a shape (only fence
  gates list several, and no pattern leads to the gate shape).
- Rules by block type list; no shipped template has one.
- Extra blocks an answer carries.

## Checked against the official server

Stairs placed next to each other with a recorded place chain turned to
each yaw (seven layouts: corners both ways, inverted corners, rows, a
ring of four): the same blocks and rotations after every placement.
Roofs the same way, on two levels (rows, a ring, a valley, corners on
the upper level, a top piece over four slopes, a wide roof): the same
blocks, rotations and batches.

Templates the same way: fences (corners, T pieces, a cross, with the
pieces placed at different yaws), chests growing into double chests
along both axes, bookshelves in a row, village walls stacked three high,
rails (a cross of straight pieces turning into corners and T pieces) and
doors side by side (the second turns round to make a double door): the
same blocks, rotations, part cells and batches after every placement.

Removal the same way: a column of three village walls (bottom, middle,
top) broken from the top with recorded hatchet swings, one piece after
the other: the same cells change to the same pieces after each break.
The break sound comes before the block's particle on both. (Block
changes travel on another stream than sounds, so their order against
sounds is not compared.)
