<p align="center"><img src="assets/icon/aerie.svg" width="128" alt="Aerie icon: a light blue feather"></p>

# Aerie

Aerie is an independent, from-scratch Rust implementation of a server and a client
that interoperate with Hytale, aimed at running on Linux, Windows and phones.

It is a fan project. It is not affiliated with, endorsed by or connected to
Hypixel Studios. "Hytale" is a trademark of Hypixel Studios Canada Inc.

## What this repository contains

Only code and documentation written by Aerie contributors.

- **No game code.** Nothing here is copied from the official server or
  client. Aerie is an independent reimplementation: we study how the official
  server behaves (reverse engineering for interoperability, as the game's EULA
  allows), describe it in our own words in [`docs/spec`](docs/spec), and write
  our own code with our own structure.
- **No game files and no game data.** Assets are read at run time from the
  player's own installation and are never shipped here or in releases. The
  machine-readable protocol and asset tables the build needs are generated on
  your machine from your own copy of the game (see "Building"); they are not
  in this repository.
- **You need to own the game** to build or use Aerie.

Aerie has not accepted, and does not use material from, the game's Shared
Source programme.

## Building

You need Rust, Python 3, and the game installed (or the official server
download). Once, before the first build, generate the protocol and asset
tables from your own copy:

```
python tools/gen.py
cargo build --release
```

`tools/gen.py` finds the game in its default install folder (or pass
`--server-jar` and `--assets`), uses the game's bundled Java runtime, and
writes `gen/`, which git ignores. It reads the server's protocol classes with
the Vineflower decompiler (downloaded once from Maven Central and checked
against a pinned hash) in a temporary folder, keeps only the facts (names,
types, offsets, packet ids) and deletes the rest. Run it again after a game
update.

## What Aerie does not do

- It does not bypass authentication, anti-cheat or any access control.
- It does not give a player any advantage over the official client.
- It sends the same kind of traffic the official client sends.

## Layout

| Path | What |
| --- | --- |
| `crates/aerie-assets` | Reads the player's Assets.zip and builds registries |
| `crates/aerie-auth` | Hytale account login and session-service calls |
| `crates/aerie-protocol` | Wire format and packets |
| `crates/aerie-server` | Dedicated server |
| `docs/spec` | Behaviour specs the code is written from |

## Status

The server logs in with a game account, completes the authenticated login and
setup with the release client, builds all setup registries from the player's
`Assets.zip`, and lets players into a generated world: breaking and placing
blocks, inventory, crafting, chat, other players. Survival mechanics, fluids,
light, creatures and real world generation are next. No Aerie client yet.

Contributions: never paste decompiled code or captured game data into issues,
pull requests or commits. Describe behaviour in your own words.

## License

Aerie is **source-available**: the code is public, but it is not "open
source" in the strict sense, because selling it is not allowed. The license is
the [Apache License 2.0 with the Commons Clause](LICENSE).

In plain words (the [LICENSE](LICENSE) file is what counts):

- **You may** read, build, use and change the code, fork it under the same
  terms, contribute back, and run your own servers with it.
- **You may** make videos, streams and guides with it, monetized or not.
- **You may not** sell Aerie or a product or service whose value comes mainly
  from it: no paid copies or paid forks, no paid hosting of Aerie as a service.
- Running a server is also subject to the game's own rules for servers.
