# 30 - Game mode types

What the official server does when a player's game mode changes, and what
Aerie does the same way. In prose: the reference was read, nothing is
copied from it.

## What the official server does

**A game mode type is an asset.** One file per type (folder
`Entity/GameMode`: Adventure, Creative, Spectator and others). Every field
is optional, and a field that is not set means "leave the player as it
is". A type can hold:

- an entity effect, applied on entering and removed on leaving;
- markers: invulnerable, intangible, no inventory access, no emotes;
- a movement configuration, a flying state and a no-clip state;
- whether trigger blocks (and block contact damage) work on the player;
- interaction overrides: root interactions bound to interaction types, and
  a flag that makes every type the map does not name run nothing;
- a spectator flag, a camera lock, visible HUD parts, a voice channel,
  whether dropping items is allowed, a death screen note;
- the interactions to run on entering: a root interaction, which in the
  assets of the two plain modes is written inline.

The two plain modes (Adventure, Creative) have a type each with nothing but
the interactions on enter. Adventure's plays a sound on the player and
around it; Creative's applies the entity effect `Creative` (0.2 seconds,
particles, a tint, sounds; its asset sits in `Entity/Effects/GameMode`).

**Entering a type** (a player in a persistent type, such as a spectator;
the plain modes are not entered this way) does, in this order: movement
(the type's configuration, then flying, then no-clip), the markers (newly
preventing emotes stops the emote playing), the spectator component and
HUD, the interaction overrides, trigger blocks, HUD parts, voice channel,
camera lock, the entity effect (unless active already), and last the chain
of interactions on enter.

**Leaving a type** undoes what the type set: the player's own movement
settings again, flying off (unless the player's mode is creative),
no-clip off, invulnerable back to what the mode says, the other markers
off, the overrides out, trigger blocks on, the HUD and voice reset, the
entity effect removed; then the interactions on enter of the mode the
player is in run.

**Changing the plain game mode** (`/gamemode`, any other way to set the
mode; nothing happens when the mode is the same, the command says so):

1. The new mode is stored and the player gets the mode packet.
2. The player's movement settings are rederived and sent: flying is
   allowed in creative and disabled otherwise, unless the movement
   configuration names a fly mode.
3. The invulnerable marker is put on (creative) or taken off. A creative
   player may also get the "respond to hit" marker, if its settings say so
   (off by default).
4. The creative eraser tool is refreshed; the types the player is in (if
   any) are reasserted; the world map settings are sent again.
5. Unless the player is in a persistent type, the interactions on enter of
   the new mode's type run.

The command then answers with its success line. The interactions on enter
run as a **server chain of type GameModeSwap** that needs the client:
the player gets the first state of the chain (a chain id counting down
from -1, the root interaction of the type, no target) in the next
packets, and the chain ends when the client has answered or is given up
on (a cancel packet follows). Players around see the chain's interaction
start and end, as for any chain.

What the player and the others are sent, in order, for creative:

1. the mode packet, the movement settings (fly allowed), the command's
   reply, the world map settings;
2. the chain's first state;
3. in one entity update of the player, the same tick: the invulnerable
   marker and the effect added (0.2 seconds left). Viewers get the same
   update;
4. some ticks later (the effect lasting 0.2 s of real time, so 4 to 6
   ticks) the effect's removal;
5. later, the chain is cancelled.

For adventure: the mode packet, the movement settings (fly disabled), the
reply, the world map settings, the chain's first state, an entity update
with the invulnerable marker removed (as a removed component, with no
update), and the chain's cancel.

## In Aerie

`gamemode.rs` reads the types from a server table (`game-modes`, one line
per type, built from `Entity/GameMode` by `aerie-assets`:
`registries/game_modes.rs`) and holds, for each type, what it sets
(effect index, markers, movement, flying, no-clip, trigger blocks,
overrides, interactions on enter). The movement settings of a creative and
of an adventure player (two payloads, with fly allowed or disabled) come
with the extras too.

A `/gamemode` that changes the mode: sends the mode packet, the movement
settings, the invulnerable marker (creative) or its removal (adventure),
the reply, and starts the type's interactions on enter as a server chain of
type `GameModeSwap` that needs the client (`start_server_synced` in
`chains.rs`). The chain runs through the normal chain machinery, so the
effect (`ApplyEffect`) and the sound (`Simple`) come out of the existing
interaction code; the effect is applied by the effects module, and its
update leaves with the invulnerable marker in one entity update, to the
player and to viewers.

`Type::apply` and `Type::unapply` list, in the official order, what
entering and leaving a type does to a player. Nothing in Aerie enters a
type of its own (no spectator command), so they have no caller yet; they
are tested alone.

## Not done yet

- The world map settings packet after a change (and at join): Aerie sends
  no world map settings at all; it belongs with the world map.
- Applying the steps of `Type::apply` that need systems Aerie does not
  have: intangible and prevent-inventory/emote markers, no-clip, trigger
  block processing, interaction overrides, movement configurations by name,
  HUD parts, voice channels, camera lock, spectating, item drop rules.
- "Respond to hit" for creative players and the creative eraser refresh.
- A player who joins in creative should get the invulnerable marker in
  its entity view; the join is not changed here.
- Death and hardcore rules, entering a type on death.

## Checked against the official server

The same script on both servers (adventure to creative, creative again,
back to adventure), one probe acting and one watching:

- the actor's packets: mode packet, movement settings (decoded payloads
  equal, fly allowed and disabled), reply, chain first state (`chains.py`:
  same), the entity update with the invulnerable marker and the effect add
  (same components, same order), the effect removal, the marker removal
  and the chain cancels, in the same order;
- the watcher gets the same entity updates for the actor (marker and
  effect together, effect removal, marker removal) and the same chain
  interaction packets;
- the effect lasts 4 to 6 ticks on the official server (it follows real
  time) and 6 on Aerie; the effect packets and their order match;
- the second `/gamemode creative` sends only the "already in this mode"
  reply on both;
- difference left: the world map settings packet (above).
