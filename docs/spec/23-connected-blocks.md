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

## In Aerie

`connected.rs` answers for a cell through a small view of the world; the
world applies the answers after a placement, a break and a harvest.
Rule sets come with the block types' registry packets.

## Not done yet

Roofs (hollow and regular shapes, valleys, toppers, widths); the
templated pattern rules of walls, fences, doors, chests, rails and
bookshelves; the "update, place, update" and "ignore updates" modes;
extra blocks an answer carries.

## Checked against the official server

Stairs placed next to each other with a recorded place chain turned to
each yaw (seven layouts: corners both ways, inverted corners, rows, a
ring of four): the same blocks and rotations after every placement.
