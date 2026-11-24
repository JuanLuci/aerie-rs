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

```
client                                  server
  ViewRadius (32), SetupFinalize (33) ─▶
          ◀── CommandTreeSync, AddToServerPlayerList, VoiceConfig, ServerTags,
              UpdateClientRuleVolumes
          ◀── JoinWorld (104)
          ◀── server opens two unidirectional streams: Chunks, then WorldMap
  ClientReady (105) ready for chunks ──▶
  ClientReady (105) ready for gameplay ▶
          ◀── SetClientId, game mode, movement, inventory, time, weather, …
          ◀── chunks on the Chunks stream, map on the WorldMap stream
```

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

Aerie sends the same `UpdateFeatures` at the same place, and now sends
`ServerTags` with the one tag before `JoinWorld`. When setup is replayed from
a capture, the capture's own packets are used instead. Aerie does not yet
send `CommandTreeSync`, `AddToServerPlayerList`, `VoiceConfig` or
`UpdateClientRuleVolumes`, so `ServerTags` is the first packet before
`JoinWorld` instead of the fourth.

### Checked against the official server

A probe joined both servers; `ServerTags` and `UpdateFeatures` are
byte-identical, and the packets around them (`JoinWorld`, `SetTimeDilation`,
`UpdateSunSettings`) follow in the same order. Live registration of features
after start-up is not done and not compared.
