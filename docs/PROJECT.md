# How Aerie is laid out

Read this before changing anything: it is the project's conventions in
one place, so you do not have to work them out from the code.

## Crates

| crate | what it holds |
|---|---|
| `aerie-protocol` | packet framing, the wire encoding and every packet type (`proto::*`, generated at build time); the `dump` example decodes captures |
| `aerie-assets` | reads the player's `Assets.zip`: the registry packets clients get (`registries/*.rs`, one file per registry) and the *server tables* (text lines the server needs beyond the packets) |
| `aerie-auth` | accounts and tokens for the server login |
| `aerie-server` | the server: connections, the world, every gameplay system, and the probe |

## The server (`crates/aerie-server/src`)

- `main.rs`: start-up, the world's tick thread, the probe's options.
- `world.rs`: the `Hub` (players, blocks, items, entities) and the tick
  (`Hub::tick`: effects, chains, support checks, processing, falling
  blocks, flush, health, stats, items, crafting). Most systems hook in
  here. **Keep a system in a module of its own** and add only the hook
  to `world.rs`.
- One module per system: `chains.rs` (interaction chains), `effects.rs`,
  `stats.rs`, `support.rs` (block rules and the physics table),
  `connected.rs`, `falling.rs`, `items.rs`, `gather.rs` (breaking and its
  table), `crafting.rs`, `processing.rs`, `inventory.rs`, `terrain.rs`,
  `chunk.rs`, `join.rs`, `login.rs`.
- A module that needs the world reads it through a small trait it
  defines (`connected::Cells`, `chains::Host`, `items::Blocks`), and
  `world.rs` implements it. Pure logic stays testable without a world.
- `probe.rs`: a scripted client used to compare servers. Changes to it
  go in a pull request of their own.

## Server tables

What the server needs from the assets but clients are not sent is
written by `aerie-assets` as text lines (`KIND key values…`), cached at
start-up, and parsed by the module that uses it (for example the
physics table: `aerie-assets/src/registries/physics.rs` writes it,
`support.rs` reads it). Add a new line kind rather than a new file. The
cache key covers the code that builds the tables, so a change rebuilds
them by itself.

## Specs and comparisons

- `docs/spec/NN-name.md`, one per system, in prose: what the official
  server does, then "In Aerie", "Not done yet", "Checked against the
  official server". Extend the spec of the system you work on; start a
  new number for a new system.
- `tools/diff/*.py` read probe captures (through `cap.py`, which runs
  the `dump` example once per capture) and print what is compared, so
  the outputs of both servers can be diffed: `cells.py` (block changes
  and sounds), `chains.py` (chain answers; `--final` ignores tick
  alignment), `effects.py`, `stats.py`, `items.py`, `falling.py`.

## Conventions

- Most files are CRLF in the git index (`git ls-files --eol`); keep
  each file's line endings. New files: CRLF.
- `cargo fmt` before every commit; `cargo test` must pass.
- Build while iterating with `cargo build --profile fast`; `--release`
  only to measure performance.
- Comments say what and why in plain words; names follow the module
  around them.
- Commits, specs, issues and pull requests in English.
