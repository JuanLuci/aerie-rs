# 03 · Login and setup

All of this runs on the game stream. Each stage has its own timeout; a client
that sends a packet the current stage does not expect is disconnected.

## Version check

The first packet must be `Connect` (0). The server compares the protocol CRC
with its own:

| Constant (build 0.7.7) | Value |
| --- | --- |
| protocol CRC | `0xE8F58B91` (as i32: −386 561 135) |
| protocol build number | 258 |

On a mismatch the server closes with `ClientOutdated` when the client's build
number is lower than its own and `ServerOutdated` otherwise. If no `Connect`
arrives at all, the server closes silently.

## Auth modes

The server runs in one of three modes:

- **authenticated** (default): the client must send an identity token in
  `Connect`. Flow in [Authenticated login](#authenticated-login).
- **insecure**: no account check. The player states who they are.
- **offline**: same as insecure, only allowed for a singleplayer world, and
  only for the world's owner.

## Insecure / offline login

```
client                          server
  Connect ───────────────────────▶
          ◀─────────── RequestInsecurePlayerOptions (364, empty)
  InsecurePlayerOptions (363) ──▶   uuid (required), username (ASCII ≤ 16, required), skin (optional)
          ◀─────────── ConnectAccept (14)   optional password challenge
  [PasswordResponse (15) ───────▶]  only if a challenge was sent
          ◀─────────── PasswordAccepted (16) | PasswordRejected (17)
  → setup
```

- The server checks the skin if one is sent and rejects invalid ones.
- **Server password:** if the server has a password, it sends 32 random bytes as
  the challenge in `ConnectAccept`. The client answers with
  `SHA-256(challenge ‖ password as UTF-8)`. On a wrong answer the server sends a
  fresh challenge and the attempts left in `PasswordRejected`. When attempts run
  out, it disconnects. With no password, or for a singleplayer world's owner, no
  challenge is sent and setup starts right away.

**Observed with the release client (0.7.7):** it answers
`RequestInsecurePlayerOptions` and receives `ConnectAccept`, then drops the
connection and shows "Server requires development mode which is not supported
in this build". In practice, release clients only complete the authenticated
login. They send their identity token in `Connect` even to an insecure server.

## Authenticated login

Mutual: the server proves its identity to the client too. Both sides use
Hypixel's session service at `https://sessions.hytale.com`.

### What the server needs before anyone joins

The server is logged in to a Hytale account and holds, for that account:

- a **session token** (bearer token for the session service), and
- a **server identity token** (a JWT with scope `hytale:server`),

both returned by `POST /game-session/new` (body `{"uuid": "<profile uuid>"}`,
bearer: an OAuth access token for the account) and kept fresh with
`POST /game-session/refresh`. The OAuth login uses
`https://oauth.accounts.hytale.com/oauth2/` (device or authorization-code
flow, scopes `openid offline auth:server`); the profile list comes from
`https://account-data.hytale.com/my-account/get-profiles`.

The server's **audience** is its session id (a UUID it holds for the session).

### Per player

```
client                                    server                          session service
  Connect (identity token) ──────────────▶
                                            check identity token
                                            POST /server-join/auth-grant ───▶  {identityToken, aud}
                                                                         ◀───  authorization grant
          ◀──────────── AuthGrant (11)      grant + server identity token
  AuthToken (12) ────────────────────────▶  access token + server authorization grant
                                            check access token
                                            POST /server-join/auth-token ───▶  {authorizationGrant, x509Fingerprint}
                                                                         ◀───  server access token
          ◀──────────── ServerAuthToken (13) server access token + optional password challenge
  [PasswordResponse ...]                    as in insecure mode
  → setup
```

Checks the server makes:

- **Tokens are JWTs signed with EdDSA (Ed25519).** Keys come from
  `/.well-known/jwks.json` on the session service. Issuer must be the session
  service URL; clock skew allowed is 300 s.
- Identity token: has a subject (the player UUID) and a username, and the scope
  `hytale:client` (`hytale:editor` for the editor client).
- Access token: audience is the server's audience; same subject and username as
  the identity token; and its `cnf.x5t#S256` claim must equal the client's TLS
  certificate fingerprint: SHA-256 of the certificate's DER bytes, base64url
  without padding. This ties the token to the QUIC connection.
- The client must send a server authorization grant ("mutual auth required");
  the server exchanges it, giving its own certificate fingerprint, and hands the
  resulting token to the client so the client can check the server.
- The skin comes from the identity token's `skin` claim (JSON).

Both session-service calls send `Authorization: Bearer <server session token>`.

## Setup

```
          ◀─────────── WorldSettings (20, zstd)   list of required common assets
          ◀─────────── ServerInfo (223)           name, MOTD, max players, fallback server
  RequestAssets (23, zstd) ─────▶                 assets the client is missing
          ◀─────────── asset parts, registry updates (Update*), translations
          ◀─────────── WorldLoadProgress (21)
          ◀─────────── WorldLoadFinished (22)
  [ViewRadius (32)] ────────────▶                 view distance in blocks, may arrive any time
  SetupFinalize (33) ───────────▶                 the player joins the world
```

- The server can answer setup with a `ClientReferral` (18) instead, which sends
  the client to another server.
- If the same account is already on the server from another origin, the new
  connection is refused. From the same origin, the old session is kicked.
- The view radius from the client is converted to chunks as ceil(blocks / 32);
  the default is 6 chunks.
- `SetupFinalize` before `RequestAssets`, or sent twice, is an error.
- **The registry updates are the big part:** block types, items, sounds,
  particles and so on, built from the game's asset files. They get their own
  spec.
- **Observed (release client 0.7.7):** sending `WorldLoadFinished` without the
  registries makes the client fail with "We have not received the asset types
  of …" and leave with a `ClientDisconnect` of type Crash. The types it
  requires before the world loads:

  AmbienceFX, BlockHitboxes, BlockTypes, Environment, FluidFX, Items,
  ItemCategories, ParticleSpawners, ParticleSystems, ResourceTypes, Weather,
  Translations, FieldcraftCategories, Trails, EntityEffects, BlockParticleSets,
  ItemAnimations, SoundEvents, Interactions, RootInteractions,
  UnarmedInteractions, BlockSoundSets, ItemQuality, ItemReticles,
  HitboxCollisionConfig, RepulsionConfig, ModelVFX, EntityUIComponents,
  EntityStatTypes, SoundSets, BlockBreakingDecals, BlockSets, AudioCategories,
  Fluids, Recipes, TagPatterns, ItemSoundSets, ReverbEffects, EqualizerEffects,
  PhysicalMaterials, MusicContainers, AudioStates, ConnectedBlockRuleSets,
  MusicTransitions, Stingers, Beams, AbilityKeywords.

The whole authenticated login and setup above was checked against the release
client with Aerie's server on 2026-11-21: identity token, `AuthGrant`,
`AuthToken` (certificate binding included), `ServerAuthToken`, `WorldSettings`,
`ServerInfo`, `RequestAssets`, up to `WorldLoadFinished`.
