# 05 · Setup contents and joining the world

Observed on server build 0.7.7 with Aerie's probe (`aerie-server probe`), a
minimal client that goes through setup and joins.

## What setup sends

After `RequestAssets`, the server sends 52 registry packets (ids 40–94) and
`UpdateTranslations` (64), then `WorldLoadProgress` and `WorldLoadFinished`.
In one capture they add up to about 18 MB; the largest are block types
(5.9 MB), items (2.7 MB), particle spawners, interactions, item player
animations, translations, root interactions and sound events.
`UpdateInteractions` (66) is sent twice.

`WorldSettings` (20) lists every common asset the world needs (path and
SHA-256), about 3.5 MB. The client requests only the ones it lacks.

**Checked:** the release client given exactly these registries finishes
loading, sends `ViewRadius` and `SetupFinalize`, and waits for the world. So
the registries are all that setup needs.

## Joining

The order and timing below are what the official server does from the
moment the client's `SetupFinalize` arrives (observed with the probe, and
read in the server's join code).

```
client                                  server
  ViewRadius (32), SetupFinalize (33) ─▶
          ◀── CommandTreeSync, ServerTags, AddToServerPlayerList, VoiceConfig,
              UpdateClientRuleVolumes
          ◀── JoinWorld (104)                       (about 10 ms after setup)
          ◀── server opens two unidirectional streams: Chunks, then WorldMap
  ClientReady (105) ready for chunks ──▶            (the client's own pace)
          ◀── ViewRadius (Chunks stream), then on the game stream the world
              settings and the player (below), chunks on the Chunks stream,
              map on the WorldMap stream
  ClientReady (105) ready for gameplay ▶            (the client's own pace)
          ◀── ClientTeleport (spawn), list update (spec 27)
```

- **Before `JoinWorld`** the game stream carries: the command tree, the
  client tags, the first player-list packet (spec 27), the voice settings
  (voice on, Opus, 48 kHz, one channel, hearing distance 32, reference
  distance 4, 60 packets a second) and an empty list of client rule
  volumes. Two threads write the tags and the list, so on some joins the
  list comes first.
- **`JoinWorld`** follows at once: nothing waits for the world to be loaded.
  On the first join it asks the client neither to clear the world nor to
  fade, and gives the section range 0 to 9 (a world of ten sections).
- **After `ClientReady` for chunks**, and not before (up to 30 s), the server
  sends, in this order: `ViewRadius` (on the Chunks stream), `SetEntitySeed`,
  `SetClientId`, time dilation, features, sun settings, post-processing
  settings, gameplay configuration, update rate; then, as the player enters
  the world: movement states, its own entity, game mode, the builder tools'
  sound set, the inventory, the three active slots, the block placement
  override (off), the spawn particles, the memories feature (locked) and
  count (0), movement settings, visible HUD components, known recipes, time
  settings and the time; then the join message (to everyone), `ClearWorldMap`
  (WorldMap stream), `ArgCacheInvalidation` for the player argument type (also
  to every other player), the world map settings twice, and a network tick.
  The tick after brings the tools section of the inventory once more (spec
  14), then the player's own entity again, and the pings (spec 27). Chunks
  follow on their own stream.
- **The spawn teleport** is not part of that: the server sends the first
  `ClientTeleport` only when the client says it is ready for gameplay (the
  probe sends it 1.5 s after `JoinWorld`), in the same tick the second
  player-list packets go out. A gameplay-ready that arrives earlier is
  remembered until the player is in the world. The movement a client sends
  before that is ignored.
- Between `WorldLoadFinished` and `JoinWorld` the official server needs
  about 10 to 30 ms (the command tree is the largest part); from `JoinWorld`
  to the player packets, about 10 to 25 ms with the probe, which answers
  `JoinWorld` at once.

### Streams

- **Stream order.** Aerie opens Chunks first, then WorldMap. QUIC numbers
  streams in opening order, and Aerie's probe accepts them in that order
  whichever carries data first; that the release client tells them apart the
  same way is not checked yet.
- **The streams carry no header.** The server opens them right before
  `JoinWorld`; each packet already names its channel in the packet table.
- Without `ClientReady` the server gives up on the join and disconnects with
  "Exception when adding player to world".
- `ClientReady` is one byte of flags: bit 0 ready for chunks, bit 1 ready for
  gameplay. The client sends the chunk flag first and the gameplay flag later,
  in separate packets.

### First 20 seconds in the world (one capture)

| Stream | Packets |
| --- | --- |
| Chunks | `SetChunk` ×61 (2.9 MB), `SetFluids` ×60, `SetChunkEnvironments` ×60, `SetColumn` ×17, `SetChunkHeightmap` ×16, `ViewRadius`, `ServerSetBlock`, `ServerSetBlocks` |
| WorldMap | `ClearWorldMap`, `UpdateWorldMap` ×3 |
| Game | `SetClientId`, `SetGameMode`, `SetMovementStates`, `UpdateMovementSettings`, `UpdatePlayerInventory` ×2, `SetActiveSlot` ×3, `EntityUpdates` ×5, `SetEntitySeed` ×4, time, weather, sun, post-processing, update rate, time dilation, features, known recipes, memories, HUD, builder tools, `ServerMessage`, `SpawnParticleSystem`, `NetworkTick`, `Ping` |

The server pings once a second (spec 27); the probe answers with a `Pong` (4) copying
the ping's id and time.

## Client features and tags

Four built-in movement abilities (crouch slide, mantling, safety roll and
sprint force) have no settings of their own to send. Each one only switches
on a client feature when the server starts. Three of them (crouch slide,
mantling, safety roll) also register the client tag `Allows=Movement`;
sprint force registers no tag. Registering the same tag again changes
nothing.

- **Features.** The world keeps one on/off flag per client feature, all on
  from the start: the five features are split velocity, mantling, sprint
  force, crouch slide and safety roll, numbered 0 to 4. Every join gets
  `UpdateFeatures` (31) listing all five as on, in number order. It comes
  right after `SetTimeDilation` and before `UpdateSunSettings`, after
  `SetClientId`.
- **Tags.** Before `JoinWorld`, after the player list and `VoiceConfig` and
  before `UpdateClientRuleVolumes`, the server sends `ServerTags` (34) with
  every client tag it knows. There is exactly one, `Allows=Movement`, with
  index 0 (the first tag registered, before any asset tag).
- A feature or tag registered while players are online is sent to them at
  once (a new `UpdateFeatures`, or `ServerTags` with the full list). Nothing
  does that after start-up, so a normal join never sees it.

### In Aerie

Aerie sends `UpdateFeatures` at the same place, and `ServerTags` before
`JoinWorld`. When setup is replayed from a capture, the capture's own
packets are used instead.

The join (`join.rs`) follows the order above. The first player-list packet
is written right after the tags, `VoiceConfig` and the empty rule volumes
after it, then `JoinWorld`, all within a few milliseconds of
`WorldLoadFinished`. Nothing else is made before the client answers: the
player's packets (about a hundredth of a second of work) are built while
waiting for `ClientReady`, and sent as soon as it says it is ready for
chunks, in the official order. The join message is sent there (it used to
be sent when the connection was accepted). Terrain is made and sent last,
column by column, nearest first, while the connection keeps answering the
client: making the 225 columns takes about 1.2 s of work (a `fast` build; it
was the 800 ms Aerie used to spend before `JoinWorld`), so the first chunks
arrive after the in-world packets and the last about a second and a half
later. The official server's chunks also stream over time.

The spawn teleport waits for the gameplay flag of `ClientReady`; the list
update of spec 27 comes with it.

### Not done yet

- `CommandTreeSync` (the command tree, about 250 KB), the builder tools'
  sound set, the spawn particles (from the gameplay configuration's spawn
  settings) and `BuilderToolsEnabledTools` after the teleport are not sent.
- The world map settings, post-processing settings, voice settings and
  memories are Aerie's constants (the default values), not read from a
  world configuration.
- `ArgCacheInvalidation` is also sent by the official server when a player
  leaves; Aerie does not.
- The official server makes a client that never sent `ClientReady` ready
  after 10 s (spec 27); Aerie waits 30 s for the chunk flag and then sends the
  world, and never makes the client ready.
- A player that joins sees the other players' entities after its own;
  the official server sends them in one update in tracker order.
- The change to `JoinWorld` (no clear, no fade, sections 0 to 9) is the
  official server's, and has not been tried with the release client.

### Checked against the official server

Two probes (`Prober2`, then `Prober2b` 4 s later, each waiting 10 s in the
world, ready for chunks at once and for gameplay 1.5 s after `JoinWorld`) on
both servers; the game stream from `WorldLoadFinished` to the first
`ClientTeleport` compared packet by packet (pings, network ticks and the
official world's animals and periodic time packets left out).

- Same order: tags, list, `JoinWorld`, then every packet listed above with
  the same ids in the same places, from `SetEntitySeed` to the second
  `UpdatePlayerInventory` and the entity update, then `ClientTeleport` and
  the second list packets. The second player's list packets and join message
  (`tools/diff/playerlist.py`) are in the same ticks as the official ones.
- Timing: `WorldLoadFinished` to `JoinWorld` 1 ms (official 8 to 32 ms),
  `JoinWorld` to the player packets 1 to 4 ms (official 10 to 25 ms), first
  `ClientTeleport` 1.50 s after `JoinWorld` (official 1.52 s). Before, Aerie
  needed 0.8 to 1.1 s before `JoinWorld`.
- Different: `CommandTreeSync`, the builder tools sound set and the spawn
  particles are missing (above); `JoinWorld` carries Aerie's world id.

`ServerTags` and `UpdateFeatures` are byte-identical on both servers.
Live registration of features after start-up is not done and not compared.
