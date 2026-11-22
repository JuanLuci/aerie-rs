# 24. Falling blocks

A block whose support drop type is *Fall* (spec 10) does not break when
it loses its support: it leaves the world and comes down as an entity,
and what happens where it lands is the block's own setting.

## Starting to fall

When block physics finds such a block unsupported:

1. A block of no material never falls.
2. The block is set to empty, as a plain change: the *break* particle of
   the block at the middle of the cell, no sound, no drops. The cells
   around its shape are marked for support checks.
3. An entity appears at the middle of the cell (+0.5 on each axis) with:
   - the block it shows, at scale 1;
   - the block's rotation, with its yaw turned half around (yaw + π);
     its look direction is level (all zero);
   - no speed;
   - physics values mass 0.5 and drag coefficient 0.05, gravity not
     inverted;
   - a collision box: the bounding box of the block's hitbox for its
     rotation, moved by −0.5 on each axis so that it sits around the
     entity's position;
   - the hitbox collision config the block's falling settings name, when
     they name one.

## Falling block settings

A block type may carry falling block settings: the impact (what landing
does: *Place*, *Break* with a "drop items" flag that defaults to true, or
*Explode*) and the name of a hitbox collision config. Without settings,
or without an impact, the impact is *Place*.

## Every tick

In this order:

1. **Move.** The step is the speed times the tick length. A step whose
   squared length is under 1e-10 is no move and clears the speed.
   Otherwise the box is swept along the step through the solid blocks
   (the sweep of spec 11 with this box):
   - nothing in the way: the entity is moved by the whole step;
   - something in the way: the entity is put where the box touches it
     and its speed is cleared (nothing bounces). When what it touched is
     a top face, the entity is *on the ground*. Touching anything else
     leaves it resting there, not on the ground.
2. **Gravity**, the rule of spec 11 with these physics values and the
   horizontal area of the box (width × depth): the terminal speed is
   −√(64 · mass · 1000 / (0.001225 · area · 1 000 000 · drag)); the
   acceleration is 32 · (−1 − sign(speed) · |speed / terminal|³); the
   speed changes by acceleration × tick length, never past the terminal
   speed.
3. **Landing.** An entity on the ground acts by its block's impact, at
   the cell that holds its position, with the rotation it fell with, and
   is removed in the same tick.

A block that starts to fall is ticked in the tick it starts: it does not
move yet and gains its first speed.

## Landing

- **Place.** When every cell of the block's shape at the landing cell is
  empty or holds a block of no material, the block is set there (with
  its parts, particles as for any placed block, the area marked for
  support checks). Otherwise it breaks as below, dropping items.
- **Break.**
  1. For each cell of the block's shape (x, then y, then z), the block's
     *break* particle at the entity's position plus the cell's offset.
  2. The block's break sound at the entity's position, to the players in
     the sound's reach.
  3. With "drop items", and when the block has gathering settings: its
     soft drop if it has one, otherwise its breaking drop — one roll of
     the drop list, or the item — spawned as item drops (spec 11) at the
     entity's position moved by +0.5 on x and z.
  4. The block's "on break impact" interaction, when it has one, runs
     for the entity at the landing cell.
- **Explode.** The block's explosion happens at the entity's position,
  then the "on break impact" interaction as above.

## What clients receive

- The tick the block starts to fall: the block change, its particle, and
  the entity with its block, its hitbox collision config (when it has
  one) and its transform.
- Each tick it moved: its transform (position, body rotation, look
  rotation).
- The tick it lands: the impact's particles and sound, then the entity's
  removal; the drops appear as items the tick after.
- A player joining while a block falls is told of the entity in full.

## In Aerie

`falling.rs` (the entity and its tick), `falling_step`,
`falling_impact` and `falling_break` in the world; the asset side adds
each block's falling settings to the physics table.

## Not done yet

- *Explode* impacts (they wait for explosions): the block is removed
  without one. *Break* and *Place* are done; no shipped block uses
  *Place*.
- "On break impact" interactions (they need interaction chains run for
  an entity that is not a player).
- Fluids: a falling entity entering water loses a fifth of its speed
  once; Aerie has no fluids yet.
- The entity moved by anything but gravity (pushed sideways, a support
  taken away under one resting against a wall), and its saving with the
  world: a falling block that is in the air when the server stops is
  lost.

## Checked against the official server

`tools/diff/falling.py`: a block with these settings set in the air and
ticked, on both servers. The same block change and particle, the same
entity components, one transform per tick with the same rotation, the
same particle and sound at the landing point, the removal in the same
tick, and a drop at the same place when one is rolled. Tick lengths are
real time on both servers, so heights are not compared digit by digit:
the script replays the rule above over each capture and checks that the
tick length every move implies stays at a thirtieth of a second.
