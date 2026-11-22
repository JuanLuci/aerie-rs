# 19. Entity effects

Effects are timed states on an entity: a heal over time, a burn, a food
buff. The registry (spec 07) describes each one to the client; this is
what the server does with them.

## An effect

- **Duration** in seconds, or **infinite**.
- **Overlap**: what adding it again does while it is active — `Ignore`
  (default), `Overwrite` (the remaining time becomes the new duration) or
  `Extend` (the new duration is added).
- **Removal**: what removing it does — `Complete` (default: it goes),
  `Infinite` (it stops being infinite and runs out its time), `Duration`
  (its time is set to zero: it goes at the next tick).
- **Stat changes** per cycle: amounts added to stats, absolute or as a
  percentage of the stat's range, and the **cycle time** (0: one cycle
  only, at its first tick).
- **Stat modifiers** while active: additive and multiplicative amounts on
  stat maximums (spec 17: summed over all active effects into the
  `Effect_ADDITIVE` and `Effect_MULTIPLICATIVE` modifiers; recomputed
  when an effect arrives or leaves).
- **Apply conditions** (the conditions of spec 17): all must hold for the
  effect to be added, and each tick for it to stay.
- A debuff flag and a status icon (shown by the client), whether it makes
  the entity invulnerable, a damage calculator (damage over time), a model
  change.

## Adding

An effect whose conditions do not hold is not added. With an explicit
duration, or when the effect is not infinite:

- already active and infinite: nothing changes;
- already active: per its overlap rule; unless ignored, viewers get an
  add update with the new remaining time;
- otherwise it becomes active with the duration as remaining time, and
  viewers get an add update (remaining time, debuff flag, icon).

An infinite effect added without a duration becomes (or stays) active as
infinite — a fresh one with a remaining time of 1, no debuff flag and an
empty icon — and viewers get an add update flagged infinite every time.

## Each tick

The effects of an entity are ticked before its interaction chains, so an
effect a chain adds runs its first cycle the tick after. In the order the
official server's map lists them, for each active effect:

1. Its conditions no longer hold: it is removed.
2. The time step is the tick's, at most the effect's remaining time.
3. Cycles: with a cycle time, the step is added to the time since the last
   cycle, and one cycle runs for every whole cycle time in it (the rest is
   kept); without one, a single cycle runs at the first tick.
4. When at least one cycle ran, the stat changes are applied once
   (predicted by every client, as a stat change of spec 17).
5. Unless infinite, the remaining time drops by the step; at zero the
   effect is removed.

Removed effects leave completely: viewers get a remove update (time 0,
empty icon). The entity is invulnerable while an active effect says so.

The tick's time step is the real time since the last tick started, and a
tick never starts less than one step (1/30 s) after the previous one; a
2 s cycle therefore comes every 60 ticks.

## Updates

Every change of a tick goes to all viewers of the entity, the entity's
own client included, as one effects update in the tick's entity updates. A
viewer that starts seeing the entity gets all its active effects.

## Interactions

- **Apply effect** (instant): adds the effect to the target entity, for
  the interaction's duration when it has one, else the effect's own way.
- **Effect condition** (instant): fails unless the target has all (or,
  with "none", none) of the listed effects.
- **Clear effect** (instant): removes the effect per its removal rule.

## In Aerie

`effects.rs` (the book: the registry packet plus one line per effect for
removal rule, invulnerability, stat modifiers and conditions; a controller
per player). The world ticks every player's effects, then the chains, then
collects the effect updates. The tick loop sleeps until shortly before a
tick is due and spins the rest, like the official one.

## Not done yet

Damage over time (needs damage); model changes; invulnerability is kept
but nothing reads it yet; effects on other targets than the entity
running the chain (owner, target); effects are not saved with the player
nor sent to a viewer that starts seeing the entity; creatures.

## Checked against the official server

Eating an apple (a made-up client chain, `tools/diff/chains.py` and
`tools/diff/effects.py`): the same chain states to the end, the apple
taken, the same effect updates (instant heal added and removed two ticks
later, the 60 s regeneration buff with its icon), the +5 health one tick
after the add and +1 every 60 ticks, on the same ticks give or take one
(the official server itself jitters by one over a minute).
