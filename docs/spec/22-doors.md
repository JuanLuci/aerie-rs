# 22. Doors

A door is a block flagged as a door whose use interaction is the door
type (a block type of spec 18). Its positions are interaction states of
the block: closed (the default, and the two "close door" states), opened
in (`OpenDoorOut`) and opened out (`OpenDoorIn`); each state can have a
shape of its own, and a sound.

## Using a door

1. An open door closes. A closed door opens away from the player: out
   when the player stands in front of it (by the door's yaw, comparing
   the player's position with the block's centre), in otherwise. A
   horizontal door (a trapdoor) always opens in.
2. The state to switch to is named by where the door comes from: closing
   from opened in is `CloseDoorOut`, from opened out `CloseDoorIn`;
   opening in is `OpenDoorOut`, opening out `OpenDoorIn`.
3. The new state's shape must fit: each of its cells is empty, holds a
   block of no material (soft), or is part of this door. When it does
   not, a door that swings both ways tries the other way; when neither
   fits, the door switches to its `DoorBlocked` state (not when it was
   closing) and the step fails.
4. Soft blocks in the new shape's cells break, with their physics or soft
   drops at the middle of the cell's base.
5. The door switches state; the cells of its old and new shapes are
   updated.
6. The new state's sound plays at the middle of the door's closed shape.

## Double doors

The door next to it on its far side — one door width times two minus one
along the door's own x axis turned by its yaw — makes a double door with
it when it is a door of the same shape, turned the other way round, at
its base cell, in the state looked for.

- Before moving, a partner in the same state as the door must be able to
  go the opposite way; if it cannot, both ways are tried swapped, else
  the door is blocked.
- After the door moved, a partner in the opposite state of where the door
  was follows: the opposite way (a horizontal one the same way).
- The sound of a double door plays between the two.

## In Aerie

`use_door` in the world; the asset side flags doors (and horizontal
ones) and gives each interaction state's sound.

## Not done yet

Double doors are written from the rules above but not yet compared with
the official server (it needs a door placed turned the other way); the
"blocked" animation's return to the closed state; doors opened by
creatures.

## Checked against the official server

A door and a trapdoor placed, opened and closed with a recorded use
chain: the same block changes cell by cell (states, parts, cleared
cells), the same sounds at the same points, the same chain states.
