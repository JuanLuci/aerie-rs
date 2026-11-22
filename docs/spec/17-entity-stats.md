# 17 — Entity stats

Behaviour of the official server, written from reading it and checked
with captures; no code. Health, stamina, mana and the other stats: their
values, what moves them, and what clients are told.

## Stat types and values

Each stat type (`Entity/Stats`, spec 07) has an `InitialValue`, `Min`,
`Max`, `Shared` (other players see it), `ResetType` (to the initial value
or to the maximum), `IgnoreInvulnerability` and `Regenerating` rules. A
living entity has one value per type, always kept within the stat's
minimum and maximum.

Modifiers move a stat's minimum or maximum: each has a key, a target (min
or max), a kind (additive or multiplicative) and an amount. Per target,
the additive amounts are summed and added to the type's bound, then the
multiplicative amounts are summed and multiply it; the value is then kept
inside.

## Changes and what clients receive

A change is one of: add, set, minimize, maximize, reset, put modifier,
remove modifier. Changes are kept per stat until the tick's entity updates
go out, one list for the entity's own client (every stat) and one for the
others (shared stats only), merged as they are made:

- an add goes into a pending first update, add or set (its amount added to
  theirs), unless either is predicted;
- a set, minimize, maximize or reset replaces the pending change (keeping
  a first update a first update), unless either is predicted;
- modifier changes are never merged.

An add or set that changes nothing is dropped. The update lists the
changed stats in the order of the official server's map (spec 14: its
pending table, made for 16 and filled 0, 1, 2…, read into a table made for
that many entries). The entity's own client gets its update with the
tick's other entity updates; other players get theirs the same way.

A client seeing an entity for the first time gets every shared stat as a
first update with its modifiers (its own entity: every stat). A player
entering the world gets its own entity twice: with the world (spec 05),
then once more as the entity tracker shows it — model, name plate, hitbox
collision, equipment, UI components, every stat, effects, in that order.
An entity's model lists its animation sets in the official map order: the
keys as a Java hash map iterates them, put through an open addressing
table (mixed hash, linear probing, read from the last slot down) twice.

## Regeneration

Each tick, for every stat but the first, each rule in order, while the
stat is not already at the end the rule moves it towards: the rule's timer
goes down by the tick's length and, once below zero, goes up by the
rule's `Interval`, and the rule applies if all its `Conditions` hold. It
adds `Amount` (`RegenType` `Percentage`: that part of the stat's range),
times the `Amount` of each of its `Modifiers` whose conditions hold; with
`ClampAtZero`, a negative amount on a stat whose minimum is below zero
stops at zero. The rules' amounts of a tick are summed per stat and added
at once (a negative sum is dropped for an invulnerable entity unless the
stat ignores invulnerability).

Conditions (each can be inverted): alive; is a player; no damage taken for
a delay; health regeneration enabled; the player's game mode; a stat
compared with an amount (`Gte` by default); wielding (blocking), which can
skip interactions flagged to be ignored; sprinting and gliding (the
movement states the client reports); suffocating.

In the game: stamina comes back 0.3 every 0.1 s while its regeneration
delay stat is 0 and the player is not sprinting, gliding or blocking;
mana 1 every 0.3 s; health only for creatures (after 15 s unhurt) and in
creative.

## Sprinting

Sprinting costs stamina through a rule of the stamina stat itself (0.1
every 0.1 s while sprinting, never below zero). The server adds one thing:
in the tick a player stops sprinting (the movement states it reports go
from sprinting to not), the stat the gameplay configuration names as the
sprint regeneration delay is set to the configured value (−0.75), unless
it is already at or below it. This happens before the tick's
regeneration, so the update the client gets is that value plus the delay
stat's own first step back toward zero (−0.65). Stamina regenerates again
once the delay stat is back at zero.

## Equipment

What an entity wears and holds changes its stats. When its armor, held
item or active utility item changes, on the next tick: the stats a newly
held weapon or utility item names are emptied (predicted by its own
client); the armor's modifiers are summed per stat and kind into
`Armor_ADDITIVE` and `Armor_MULTIPLICATIVE` on the maximum (a broken piece
counts times the gameplay config's broken armor penalty; put again even
when unchanged, removed when no piece gives that kind); the held weapon's
own modifiers go under `*Weapon_0`, `*Weapon_1`… per stat and the utility
item's under `*Utility_…` (when the held item allows a utility item), put
only when different, the keys past the item's count and the stats it does
not touch removed.

The tick after, everyone who sees the entity gets its equipment: the four
armor ids (empty for none), the held item and the utility item (`Empty`
for none).

## Commands

`/player stats add|set|get|reset|settomax <stat> [value]` with the
official replies (`server.commands.entityStats.success`, `.valueReset`,
`.value`, `.entityStatNotFound`). `/give <player> <item>
[--quantity=N]` adds to the player's pickup sections (spec 14) and
replies `server.commands.give.gave`.

## Saved

A player's stat values are saved when it leaves and restored when it
joins.

## Not done yet

The official server ignores movement packets until it takes the client
for ready (a join detail, with the players system).
The interactions a stat runs when it reaches its minimum or maximum, and
death at no health, need interactions run on the server and the damage
system. Blocking, damage times and suffocation are never true yet.
Modifiers from effects; regeneration rules on armor pieces; armor hidden
by the player's settings; the held item follows the slot the client's
chains name until chains run on the server. Modifier maps list their keys
in insertion order.

## Checked against the official server

The same commands on both servers (`tools/diff/stats.txt` with
`tools/diff/stats.py`): the same replies and stat updates, including the
command's change merged with the tick's regeneration, a set followed by
regeneration, stamina and mana regenerating to full; the same two
own-entity packets at join; armor put on and taken off: the same
modifier updates (9, 26, 17 on health) and equipment updates a tick
later. A sprint of about 2.8 s (`probe --sprint 3 --listen 28`): the same
28 stamina steps down, the delay set to −0.65, its 7 steps up and stamina
regenerating after. Differences left: which tick a command lands in.
