# 25 - Server configuration file

What the official server keeps in its settings file, which of it Aerie uses,
and how the file, the command line and shutdown fit together. In prose: the
reference was read, nothing is copied from it.

## The file

The official server keeps one JSON object in `config.json`, in the folder it
is started from. It is versioned (the current version is 4); a file with an
older version is read, upgraded and saved again. Keys are written in
PascalCase. The settings Aerie has are:

| key | meaning | default |
|---|---|---|
| `Version` | the file's version number | 4 |
| `ServerName` | the name clients see in the server information | the official name; Aerie: `Aerie Server` |
| `MOTD` | the message of the day, sent in the same packet | empty |
| `Password` | the password asked of every player; empty means none | empty |
| `MaxPlayers` | how many players may be in the server at once | 100 |
| `MaxViewRadius` | the largest view radius, in chunks | 32 |
| `Defaults.GameMode` | the game mode a new player starts in (`Adventure` or `Creative`) | `Adventure` |

The official file holds many more keys (the default world, hardcore mode,
connection timeouts, rate limits, modules, mods, storage providers, the ban
list, backups, the world map, the update checker, a fallback server and
others). Aerie has none of those systems.

Other facts from the reference:

- A view radius outside 1 to 32 is not an error: the server warns and uses
  the nearest limit.
- A `MaxPlayers` of zero or less means no limit.
- A game mode written in a legacy style is read by name.
- The password is stored and compared as written, never hashed in this file.

## In Aerie

- The file is `config.json` in the folder Aerie is started from, or the
  path given with `--config`.
- **Missing file:** Aerie writes it with the defaults above. Only the keys of
  the settings Aerie has are written (the table, with `Defaults` holding just
  `GameMode`).
- **Existing file:** it is read. A key the file lacks is added with its
  default and the file is written again; if nothing lacks, the file is not
  touched at all. Every other key, known to the official server or not, is
  kept as it is, with its value and its place in the file, so a file from the
  official server is not damaged by running Aerie on it. A key with a value
  of the wrong type falls back to the default for the run (with a warning)
  and is left in the file as written. A file that is not valid JSON, or not
  an object, stops the start and is never overwritten.
- **Override order:** the file, then the command line for this run. Options
  are never written back. The options are `--name`, `--motd`, `--password`,
  `--max-players`, `--max-view-radius`, `--game-mode` and `--config`. An
  empty `--password` means no password. The official server's own options do
  not override these settings; the Aerie options do, as they did before the
  file existed.
- **Used for:** the name and message of the day go into the server
  information every player gets during setup; the password is the one asked
  after login; `MaxPlayers` is also in the server information, and is the
  limit below; `MaxViewRadius` times 32 is the view radius in blocks that the
  server tells each player when it enters the world (the official server
  does the same); `Defaults.GameMode` is the mode the built world starts
  players in. The file's own `Version` is not interpreted: Aerie neither
  upgrades nor downgrades it.

## A full server

The official server checks the count when a connection reaches its login
handler, before the player is asked for anything. When the number of players
already in the world has reached `MaxPlayers`, the connection gets a
disconnect packet whose message is the translation key
`client.general.disconnect.serverFull` and is closed. Aerie does the same
right after the version check, so a full server never starts a login.

## Shutdown

On Ctrl+C or SIGTERM the official server runs its shutdown: before the worlds
stop, every connected player gets a disconnect packet. The message is the
shutdown reason's own text when it has one; a signal has none, so players get
the translation key `server.general.disconnect.stoppingServer`. Then the
worlds are saved and the process ends.

Aerie: it stops accepting connections, queues that disconnect for every
player (the same key), gives the connections half a second to write it, then
saves what players carry (items, selected slots, stats, position) and the
block state with the same code that runs when a player leaves, closes the
endpoint and returns. Placed and broken blocks are written as they happen,
so they need nothing more.

## Not done yet

- The official server's other keys are only kept, not used.
- Raising or lowering `MaxPlayers` at run time (the official `/maxplayers`
  command) does not exist.
- The view radius a client asks for is not yet clamped to `MaxViewRadius`
  (Aerie streams a fixed area).
- Upgrading a file of an older version.

## Checked against the official server

- The key names, defaults and the sample file written by the official server
  were read from the reference and from a sample file: Aerie started on a
  copy of that file used its values and left it byte for byte as it was.
- The full-server and shutdown messages are taken from the reference; the
  real official packets are still to be compared on a separate official
  server.
