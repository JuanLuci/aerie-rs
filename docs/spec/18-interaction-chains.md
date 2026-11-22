# 18. Interaction chains

How the server runs the interactions a client starts (a click, a use, a
swing) and the ones it starts itself. The client predicts every chain; the
server runs the same chain next to it and the two exchange the state of
each step.

## Vocabulary

- **Interaction**: one step with a type (place a block, wait, change a
  stat, a condition…). It has a run time, effects, a rule for what happens
  when the held item changes (`Cancel` by default, `Fail`, `Finish`,
  `Ignore`), settings per game mode and its own interrupt rules.
- **Root interaction**: a named list of interactions, a cooldown, settings
  per game mode and interrupt rules. An item names one root per
  interaction type (primary, secondary, use, pick, swap from, held…); so
  can an entity; an empty hand uses the "Empty" unarmed set.
- **Operations**: a root compiled to a flat list. Each interaction adds
  itself and, depending on its type, the interactions it leads to, with
  labels and jumps between them. A plain interaction with a next and/or
  a failed interaction compiles to: itself (label 0 = where failure goes),
  the next interaction's operations, a jump to the end (only when there is
  a failed interaction), the failed interaction's operations. A jump is an
  operation of its own: it sets the counter to its target and is finished.
- **Chain**: one run of a root for an entity. It has an id (positive and
  growing when the client starts it, negative and decreasing when the
  server does: −1, −2, …, back to −1 on overflow), the interaction type,
  the data it started with (target block, target entity, target slot, hit
  location and detail, proxy entity), a context (the entity, the held item
  with its container section and slot) and a call stack of roots.
- **Operation counter**: the position in the current root's operations.
  **Operation index**: how many operations the chain has gone through; it
  only grows and it is the index the two sides use to name a step.
- **Entry**: the state of one step (by operation index): the server's
  state, the client's state when it has arrived, the time it started.
- **Step state** (what the two sides exchange for an entry): state
  (`Finished`, `Skip`, `ItemChanged`, `Failed`, `NotFinished`), progress
  in seconds, the operation counter and root it belongs to, the number of
  forks it made, the root it entered, and data only some interaction types
  fill (block position and face, entity, charge value, chosen branch…).
- **Fork**: a chain started by a step of another chain. It shares the
  chain id and is named by a fork id: (entry index, sub index), nested for
  forks of forks.

## Packets from the client

The client sends chain packets in batches. Each one names a chain id (and
fork id), and carries: whether it is the chain's first packet, the
interaction type, the equip slot, the active hotbar and utility slots, the
ids of the item in hand and of the utility item, the chain data, the
chain's overall client state, new forks, and the step states starting at an
operation index. They are queued per player and consumed in the player's
interaction tick.

Each tick the queue is read from the front:

- A packet for a chain the server has (walking down the fork ids) is
  applied to it. If a fork in the path does not exist yet, the packet is
  kept aside as temporary data for that fork, unless the fork can no longer
  appear (its sub index is below the number of forks the step already
  made, or the step is gone), in which case the packet stays in the queue.
- A packet for a chain the server does not have starts it (below). If it
  cannot start yet (blocked by rules, on cooldown, root not known), it
  stays in the queue and nothing after it is consumed this pass, except
  packets flagged as desync or with a proxy entity. A packet queued longer
  than the operation timeout is dropped with a cancel packet.
- A packet flagged as desync for an unknown chain is dropped with a
  cancel; when any packet of the pass was flagged, every queued first
  packet of an unknown chain without proxy is dropped with a cancel too.

After a pass that consumed something, all chains are ticked and the queue
is read again; this repeats until a pass consumes nothing. With an empty
queue the chains are ticked once.

## Starting a client chain

Checks, in order; each failure sends a cancel packet for the chain and
consumes the packet unless noted:

1. Not a first packet, or it names a fork, or it has no chain data.
2. Chain id ≤ 0, or not above the last client chain id accepted.
3. With a proxy entity: the entity must exist (wait up to half the
   timeout) and be reachable. Otherwise the context is made for the
   interaction type: armor slot for equipped, utility for off hand, the
   tools item when one is in use, the active hotbar item otherwise.
4. The root for the type: from the entity's own interactions, else from
   what the context holds (empty hand: unarmed set). None: cancel.
5. A root with no operations: cancel. A root the world's gameplay
   settings do not allow: cancel.
6. Rules against running chains (below): blocked → the packet waits.
   Chains it interrupts are cancelled.
7. The packet's active hotbar slot must be the server's: otherwise cancel
   and tell the client the real slot (section −1). Same for the utility
   slot (section −5).
8. The item id in hand, then the utility item id, must equal the
   server's: otherwise cancel.
9. Cooldown (below): on cooldown → the packet waits.
10. The chain is created with the packet's id and the packet is applied.
    A start event is raised; a listener can cancel (cancel packet).
11. A target block must be in a loaded section; a filler block is replaced
    by its base block (and when the two positions hold different blocks
    the filler position is cleared). The target entity (by network id),
    target slot, hit location and hit detail are remembered.
12. The id becomes the last client chain id. The chain is ticked at once;
    if it does not end, it is kept and skipped by the tick that follows in
    the same pass.

## Applying a packet to a chain

New forks in the packet are routed to the fork they name or kept as
temporary data. Without step states the packet only sets the chain's
client state. With them, for each state at index base+i: an index beyond
what can follow is ignored; when the entry exists, the client state is
set — but a state that goes back from ended to not finished, or one that
names another operation counter or root than the server's entry, flags the
entry and the chain as desynced and stops; when the entry does not exist
yet, the state is kept until the server reaches that index. Then the
chain's client state is set.

## Ticking a chain

A chain whose server state is still `NotFinished` runs operations:

1. First run of a chain: its time shift is the global time shift of its
   interaction type (a fork inherits "first run" from its parent). Later
   runs: no time shift.
2. Loop: when the counter is past the end of the root, pop the call stack
   (continuing after the operation that entered the root) until there is
   something to run; an empty stack ends the loop.
3. Run the current operation for the entry at the current index:
   - The client state kept for this index is attached. If the entry was
     flagged or the state does not fit, the chain is flagged desynced, the
     kept states from here on are dropped and the server's state is sent.
   - An operation that waits for client data and has none yet does not
     run: the server's state is sent once, and after the timeout the chain
     is cancelled as failed.
   - Otherwise the time is that of the entry (the first time: the chain's
     time shift, and the entry starts then), the operation ticks, and the
     server's state is sent when it is the first run or it changed
     (progress compared by whole seconds). Then the operation's handling
     runs (telling other players, below) and a finished entry is removed
     when the client has finished it too.
4. If the call stack changed, continue with the new root. If the counter
   did not move, the loop ends (the operation is still running). Otherwise
   the index grows by one and the loop continues.
5. After the loop the chain's server state is derived: past the end → the
   final state (that of the last entry that ended, `Finished` by default);
   else from the current entry: failed states fail the chain.
6. A chain that needs the client sends one packet with the states
   collected in this tick, starting at the index the tick began with.

Every interaction's tick is wrapped the same way: if the held item is no
longer the one the chain started with (other active slot or another kind
of item in the slot) the step ends according to its item-change rule
(`Cancel` cancels the whole chain); otherwise the step is not finished
until its run time (or its animation, when it waits for it) has passed,
then finished, the overshoot becoming the chain's time shift; then the
type's own logic runs; a step still running is skipped when the client
says so and skipping on click is allowed. When the step ended and the
type's logic did not move the counter or the call stack, the counter
advances by one.

A chain whose server state has ended is removed once the client state has
ended too (or at once when it does not need the client): if operations
were left, the current one is told it failed; the completion callback runs.
While the server waits for the client's end, or the client has ended and
the server has not, the chain is cancelled after the operation timeout.
A fork is ticked by its parent before the parent itself and a chain stays
while it has forks.

Cancelling a chain sets both states to failed, sends a cancel packet for
it and does the same for its forks.

## Rules between chains

A new root is compared with every running chain (and their forks when
nothing is interrupted yet), first with the chain's root, then with its
current operation when that has rules: when the new root interrupts it the
chain is cancelled; else when the chain blocks it the new one cannot
start. Rules are sets of interaction types and tags (see the registry).

## Other players

When a step starts, and again when it ends, the players within its view
distance get a play-interaction packet (entity, chain id, fork id,
operation index, interaction, type, cancel flag); the owner does not when
the chain needs the client.

## Limits, timeouts, cooldowns

At most 1000 chain packets wait per player; beyond that the first packets
of chains are answered with a cancel and the rest dropped. The server's
states of a tick go out in one packet, at most 128 chains. The operation
timeout is twice the player's average tick ping plus 3 s. Chains are ticked
in the tick's damage-gathering stage; forks, then the chain.

A root's cooldown has an id (the root's id by default), a time (0.35 s by
default, 0 for collision, projectile, game mode swap, stat effect, pickup
and break types), charge times (one charge by default), and flags. The
game mode's settings can replace it. A cooldown or settings flagged
"click bypass" / "skip chain on click" on a client chain resets the
cooldown instead of checking it. Checking takes one charge and restarts
the cooldown; it is on cooldown while time remains or no charge is left.
Each tick the remaining time drops and missing charges refill one by one
(each after its own charge time); a cooldown with full charges and no time
left is forgotten. Cooldowns tick once per interaction tick, plus the
largest time shift after each pass that consumed packets. When a client
chain completes its cooldown is ticked 1/60 s more.

Default "blocked by" sets: primary, secondary, abilities, use and pick
are blocked by the standard inputs (primary, secondary, abilities, use,
pick, swap from, swap to); swap from/to by swap from/to; held, off hand,
equipped, dodge and game mode swap by themselves; the rest by nothing.

## What each interaction type does

Unless said otherwise a type compiles like the plain one (itself, next,
failed) and, after its own logic, jumps to label 0 when it failed.

- **Simple**: nothing of its own; waits for nobody.
- **Instant types**: their logic runs on the step's first run only.
- **Serial**: compiles to its interactions one after another; never runs.
- **Parallel**: enters the first root; forks one predicted chain (with a
  copy of the context) for every other root; finished. Needs the client.
- **Condition**: fails unless the owner's game mode and movement states
  (jumping, swimming, crouching, running, flying, gliding) are as asked.
- **Repeat**: forks its root (predicted). While the fork runs it is not
  finished; when the fork finishes it forks again until the repeat count
  is used up (−1: for ever), then finishes; when the fork fails it fails.
  Needs the client.
- **Replace**: on its first run, unless already failed, swaps in the
  root named by an interaction variable (details below).
- **Block types** (wait for the client): on the first run — with "use
  latest target" the client's block position becomes the target (failed
  without one); no target block: failed; out of reach: failed; section not
  loaded: failed; an empty block (id 0 or 1): failed unless the context
  remembers a removed block; else the type's block logic runs.
- **Use block**: the target block's own root for the chain's interaction
  type; none, or one the world does not allow, or a cancelled use event:
  failed. Otherwise finished and that root is entered (the step's state
  names the root entered).
- **Place mode select**: compiles to itself with six labels, the default
  branch and a jump to the end, then each other branch present (replace,
  retype, extrude, surface draw, fast place — an absent branch shares the
  default's label) with its jump, then the next interaction. It jumps to
  the branch for the player's place mode, records the label index in its
  state and is finished.
- **Drag place** (waits for the client): the client's state carries how
  many forks it made for the chain's type. A count above the gesture
  limit or below what the server already forked fails the step. Otherwise
  the server forks the missing ones, at most "blocks per tick" of them per
  tick, each a predicted fork of the fork root with a copy of the context.
  When it has caught up and the client's state has ended, the step takes
  the client's state; else it is not finished.
- **Place block** (waits for the client): first run — without a position
  and rotation from the client nothing happens (the step ends by its run
  time); section not loaded, nothing held, no container, out of reach, an
  unknown block id from the client: failed, and the step does not jump to
  its failed label. Otherwise the block is placed (spec 15) and the step's
  state carries the position and what the cell now holds (block id and
  rotation). In adventure the context forgets the held item when its last
  unit was used. Later runs: the step takes the client's state.
- **Break block** (a block type): harvest mode picks the block up when
  gathering is allowed and the block is picked up by interaction; else,
  when breaking the block is allowed (and, with "soft only", the block is
  soft): in creative the block breaks, otherwise it takes one hit with the
  held item (spec 12), over the tool's break shape when it has one. Every
  tick the step's state carries the target position and what the cell
  holds.
- **Chaining** (waits for the client): compiles to itself with one label
  per next interaction and per flag (flags sorted by key), then each
  branch followed by a jump to the end. The client's state names a flag
  index (jump to that flag's branch) or a chaining index: −1 means not
  finished yet; a valid index is remembered per player (by the
  interaction's id, or its chain id when named) with the time of the
  attack, and the step jumps to that branch; out of range fails.
- **Change active slot**: ignores item changes. First run: the slot is
  its fixed target (finished at once when that is already the active
  slot) or the chain's target slot (0 when the chain has none); the
  entity's active hotbar slot changes, and the item now held, when it has
  a root for "swap to", gets it forked (predicted) with a context of its
  own. Finished.
- **Select** (waits for the client): the entities a selector hits (the
  client names them) and the blocks its shape sweeps each fork a chain of
  their own, not predicted; with labels, hitting nothing fails the step
  when its fail-on rule says so; otherwise the step takes the client's
  state.
- **Use entity** (instant, waits for the client): the root the target
  entity gives the chain's type is entered; no entity, out of reach or no
  such root: failed.

A fork's chain data starts as a copy of its parent's (a selector's forks
start empty) and takes from the fork's context the target slot (the
parent's when the fork has none; slot 0 when neither has), the target
entity, the hit location and detail, and the raw target block.

Layouts of the types with a layout of their own, beyond the ones above:
first click (click, then held at label 0); charging and wielding (a jump
to the end right after the operation, then one branch per charge time in
ascending order, then the failed branch at the last label); memories
condition (the same, by ascending level); apply force (next, ground,
collision: three labelled branches, each with its jump); movement
condition (failed, forward, back, left, right and the four diagonals: nine
labelled branches); damage entity (failed, next, blocked, then one branch
per angle and per target in key order, an absent one being the next
branch again; every branch ends with a jump to the end).

## Types the server does not run yet

Every other type keeps its layout and its place in the chain, with a
stand-in for its logic:

- A **block type** (a door, a bench page, a block condition, changing a
  block or its state…) checks its target like any block type and ends by
  its run time; when the client's state for the step has already ended
  the step takes that verdict.
- Any **other type** follows the client: it waits for the client's state
  of the step (unless the client is the one waiting for the server),
  takes it, enters the root the client entered, makes the forks the
  client made, and when the step ends goes where the client's next step
  says it went.

## In Aerie

`chains.rs`: the book (interactions and roots from the registry packets,
plus one line per interaction from the asset side with its server type
and the settings the packets do not carry) and one manager per player,
ticked first in the world tick. The world is reached through one trait
(reach, cells, placing, breaking, opening benches, packets out). Without
the interaction registries (unit tests) the old direct path stays: a
placement or a hit applied when the client's packet arrives.

## Not done yet

- Chains the server starts on its own (held and equipped item
  interactions, the swap when the held item changes by itself, stat
  effects, deaths, collisions, projectiles) and so chains that do not
  need the client.
- Selector shapes on the server (blocks swept by a swing) and the forks
  for hit entities; damage (combat).
- Proxy entities; the world's allowed-roots list; start and use events.
- A step that waits for its animation: animation lengths are not read.
- The gesture tracker that rejects implausible drag-place positions; the
  filler cell cleared when a chain targets a stale filler.
- Tags are the ones the registry packets carry (roots: with their
  inherited tags).
- Chaining's "unexpected index" check (the official server only logs it).
- Picking a block up by using it (harvest), "soft only" breaking.
- Each type listed under "Types the server does not run yet".

## Checked against the official server

`tools/diff/chains.py` lists a server's answers to chains (state packets,
cancels, slot corrections) from a probe capture, roots by name. Replayed
on both servers, same answers:

- placing a block (recorded client chain): identical packets;
- 27 recorded hatchet swings at a block: identical for the first swings,
  then the same states spread differently over packets (which tick a
  client packet lands in); `--final` (last state per step) is identical
  for all 27, and so are the block damage, tool wear, sounds and
  particles;
- using a furnace (recorded chain): identical packets, the window opens;
- a use chain that never sends its client data: the same first answer and
  the same cancel after the timeout;
- hotbar swaps: two accepted, one naming the wrong active slot: the same
  cancel and slot correction.

The probe now tells the server it is ready for gameplay 1.5 s after
joining; sent at once, the official server missed it and took the probe
for not ready for 10 s (no timeouts, no movement).
