# 27 - Player list, ping and name plates

What clients are told about who is on the server and how far away they are
(the player list and its ping column), and about the text over a player's
head. In prose: the reference was read, nothing is copied from it.

## Measuring a player's ping

- The server sends each player a `Ping` (3) once a second, counted in world
  ticks: the first one on the first tick after the player's entity is in the
  world, then every time a countdown of 1.0 s (reduced by each tick's real
  time step) runs out. That is 30 ticks, sometimes 29 or 31. The ping carries
  a random id, the send time and the last measured value (microseconds) of
  each of the three kinds.
- The client answers each ping with up to three `Pong` (4) packets, one per
  kind: raw, direct and tick. Every ping is remembered per kind, oldest
  first. A pong is matched with the oldest unanswered ping of its kind; if
  the ids differ the pong is refused (the official server raises an error;
  Aerie ignores it). The time between sending the ping and receiving the pong
  is the measurement, kept in whole microseconds.
- Each kind keeps its measurements of the last second. Whenever a pong adds
  one, measurements older than a second (compared with the new one) are
  dropped and the average of the rest is recomputed. The "last value" in the
  next ping is the latest measurement.
- The number the list shows is the average of the **direct** kind, rounded up
  to a whole microsecond and then cut down to whole milliseconds. A player
  with no direct measurement shows 0.

## What is sent, when, to whom

| when | packet | to |
|---|---|---|
| a player connects (before it is in the world) | `AddToServerPlayerList` with every player, itself included; the joiner's own entry has no world | the joiner |
| same moment | `AddToServerPlayerList` with the joiner's entry (no world) | every other player |
| the client says it is ready for gameplay (`ClientReady`, gameplay flag) | the full list again, now with the world's id for every player in the world | the player |
| same moment | its own entry, with the world's id | every other player |
| a player leaves | `RemoveFromServerPlayerList` for it | every other player |
| every 10 s from server start, whatever else happens | `UpdateServerPlayerListPing`: the ping of every player | every player |

Entries hold the player's id, name, world id (absent until the player is in
the world), ping, a spectating flag and an optional lives count (only in
hardcore mode, which Aerie does not have). The ping in an entry is the
current list ping (0 when nothing was measured yet).

Order: the players come out in the order of the server's player table, a
hash table keyed by player id: sorted by bucket (the id's hash, spread, taken
modulo the table size: 16 slots, doubling when three quarters full), join
order within a bucket. The ping update walks a different map (open
addressing with a mixing hash and linear probing, sized for the player count,
grown at three quarters, walked from the last slot down), so two players are
often listed in the opposite order there. Aerie reproduces both orders.

The 10-second update is on a timer of its own, not tied to the tick or to
any join: the first fires 10 s after the server started, the next 10 s after
the previous one ended. Over 30 ticks per second that is 300 ticks.

## Name plates

A name plate is a text shown over an entity. A player has one from the
moment it joins, with its name. Clients are told the plate:

- in the entity's first update to a viewer (after the model);
- again whenever the text changes (nothing is sent when it is set to the same
  text), to every viewer including the player itself, in the entity update
  of the tick after the change;
- as a removal of the plate component when the plate is taken away. After a
  removal, viewers that start seeing the entity get no plate; setting a text
  again creates the plate and always sends it, even an empty text.

What changes a player's plate in the official game: the display-name
component (set only by plugins; players start with their name) and the
command `/entity nameplate TEXT --entity=ID` (with no text it removes the
plate). The command replies with the message keys
`server.commands.entity.nameplate.updated` / `.removed`. Another source of
plates is mobs (their name), which Aerie does not have.

## In Aerie

`playerlist.rs` holds the measurement (`Latency`), the list orders, the
packets and the name plate state (`Plate`); `world.rs` calls it: at join
(first list packets), at `ClientReady` (second ones, sent at the next tick),
in the tick (pings, 10-second update, name plate changes) and for the pongs.
The old 5-second ping loop of the connection is gone. `/entity nameplate`
works with `--entity=<player uuid>`.

## Not done yet

- A client that never sends `ClientReady` is made ready by the official
  server after 10 s; Aerie does not.
- The ping value could not be compared with the official server: the probe
  answers every ping with a raw pong only, so the direct average stays 0 on
  both servers. The rule is covered by unit tests only.
- A refused pong (wrong id) disconnects nothing in Aerie.
- Players hidden from each other, spectating and lives are not modelled.
- `/entity nameplate` without `--entity` should use the entity the player
  looks at; Aerie answers with the "no entity in view" message. Plates of
  other entities do not exist yet.
- Name plates are sent to every player, not only to those near enough to see
  the entity.

## Checked against the official server

Two probes (`Prober2` joins first, `Prober2b` 15 s later and leaves before
the end), on both servers, 20 s of script after the first join. The player
`Prober2` runs `/entity nameplate Hello --entity=<its uuid>`, 8 s later with
`Bye`, 8 s later with no text. `tools/diff/playerlist.py CAPTURE` prints the
list packets, pings, name plates and messages with their tick.

Same on both: the two connect-time and two ready-time list packets and their
recipients (world absent, then present); the remove on leave in the tick of
the "left" message; pings every 30 ticks (sometimes 29 or 31) with only the
raw last value set; ping updates 300 ticks apart to every player, with the
map order (second player first); name plate change and removal one tick
after the command's reply, to the player and the other probe. Different:
absolute ticks (Aerie takes longer to get a client ready), entity network
ids.
