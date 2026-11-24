# The probe

The probe is a scripted client built into `aerie-server`. It joins a
server the way a game client does (insecure mode, so it needs no account),
finishes setup, then follows a script: it moves, chats, uses blocks, moves
items, replays recorded interactions, and waits for the server's answers.
Every packet the server sends is saved, and what those packets say about
the player is kept as a *state* and written out as JSON.

Its job is comparison: run the same script against the official server and
against Aerie, then diff the two runs (`tools/diff/*`). The probe never
needs to know what is right; it only has to do the same thing on both
servers and report everything.

```text
aerie-server probe --upstream 127.0.0.1:5520 --capture run-a --name Prober --script chest.txt
```

## Options

| option | what it does |
|---|---|
| `--upstream ADDR` | the server to join (required) |
| `--capture DIR` | where the run is written (`probe` by default) |
| `--name NAME` | the player's name (`AerieProbe` by default) |
| `--uuid UUID` | the player's id, to come back as the same player (random otherwise) |
| `--script FILE` | the steps to run once in the world (below) |
| `--steps "a; b"` | steps given inline, `;` between them; run after `--script` |
| `--listen SECS` | how long to keep listening: after the script (3 s by default), or after setup when there is no script (20 s) |

The older one-shot options still work, for the scenarios written with
them: `--chat "line;/cmd"`, `--goto "x y z"`, `--look "yaw pitch"`,
`--send "id:hex;…"`, `--use-block "x y z"` (a bare chain, which the
official server refuses: use the `use` step instead), `--after
"id:hex,id:hex;wait;…"` (one step every 0.7 s, 4 s after joining),
`--sprint N`, `--interactions DIR` with `--root-map FILE` (replays a
recording on the block under the spawn). A script is easier to read and
does not depend on timing: prefer it for new scenarios.

## What a run writes

| file | what it holds |
|---|---|
| `NNNN-ID-Name.bin` | every packet the server sent, in order, one file each (the payload, decompressed) |
| `index.tsv` | one line per packet: sequence, id, name, size, stream (`game` or `uniN`) |
| `state.json` | the player's state at the end, and how the script went (`script.finished`, `script.error`, `script.checks`) |
| `states.jsonl` | one line per `state` step: `{"line", "label", "state"}` |
| `roots.tsv` | this server's root interactions, `id<TAB>name`, for `replay … roots` |

`dump` (`cargo run --release -p aerie-protocol --example dump -- --capture
DIR`) prints the packets of a capture decoded.

### The state

Built only from what the server sent, with server-specific ids turned into
names (blocks, root interactions, stats, effects), so two servers can be
compared directly:

- `position`, `body`, `look`: from teleports and the player's own entity
  updates (and the probe's own `goto` / `look`);
- `game_mode`, `held` (the item in the active hotbar slot), `active_slots`;
- `inventory`: every section (`hotbar`, `storage`, `armor`, `utility`,
  `tools`, `backpack`, `abilities`, `rune_bag`) with capacity and slots
  (item, quantity, durability when it has one);
- `stats`: value per stat, with the modifiers put on it;
- `effects`: the active effects with remaining time;
- `windows`: the open windows with their type, data and slots;
- `chat`: every server message, as raw text or as the message key with its
  parameters (`key name=value …`);
- `teleports`, `disconnect` (the reason, if the server dropped the probe),
  `packets` (how many of each id), `sections_known`.

The probe also keeps the blocks of every section the server sent and every
change after it, so a script can ask for the block at a position.

## Scripts

One step per line. `#` starts a comment (at the start of a line, or after
a space). The script starts once the player is in the world for the server
(ready for gameplay and spawn teleport confirmed, about 3 s after joining)
and stops at the first step that fails to run (a step that cannot be built,
a lost connection). Then the probe listens for `--listen` seconds and
writes its files.

Sections are named (`hotbar`, `storage`, `armor`, `utility`, `tools`,
`backpack`, `abilities`, `rune_bag`) or numbered (`-1`, `-2`, …); an open
window's slots are the section `windowN` (N is the window id, usually 1).

### Actions

| step | sends |
|---|---|
| `chat TEXT` | a chat line (commands too: `chat /time set noon`) |
| `cmd TEXT` | a chat line, then waits up to 5 s for the server's reply |
| `goto X Y Z` | a movement to that position |
| `look YAW PITCH` | where the player looks (radians; body follows the yaw) |
| `slot N` | the active hotbar slot |
| `use X Y Z [FACE]` | a `Use` interaction on the block, as a client sends it: the held item's use root fails over to the block's use root. Both roots come from the registries this server sent; the block must be known (the step waits up to 5 s for its section). `FACE`: `up`, `down`, `north` (default), `south`, `east`, `west` |
| `act SECTION KIND` | an inventory action: `take_all`, `put_all`, `quick_stack`, `sort` |
| `move FS FSLOT QTY TS TSLOT` | moves `QTY` items from one slot to another |
| `close [ID]` | closes a window (the last one opened by default) |
| `send ID:HEX[,ID:HEX…]` | raw packets, together |
| `replay DIR [at X Y Z] [roots FILE]` | a recording's interaction chains, below |

### Waiting and looking

| step | does |
|---|---|
| `sleep SECS` | waits |
| `wait chat [TEXT]` | until a server message (containing `TEXT`) |
| `wait window` / `wait close` | until a window opens / closes |
| `wait block X Y Z` | until the server changes that block |
| `wait inventory` / `wait teleport` | until the inventory is sent / a teleport |
| `wait packet ID` | until a packet with that id |
| `state LABEL` | writes the state to `states.jsonl` |
| `expect block X Y Z NAME` | the block there is `NAME` |
| `expect item SECTION SLOT ITEM\|empty [QTY]` | what a slot holds |
| `expect held ITEM\|empty`, `expect window`, `expect nowindow`, `expect mode adventure\|creative`, `expect chat TEXT` | the rest of the state |

A wait counts what arrived since the last action step, so it does not miss
an answer that came before the wait started. It gives up after 5 s
(`… for SECS` to change that, e.g. `wait window for 10`); a wait that gives
up is written as a failed check. An `expect` checks the state at that
moment: put a wait before it when the answer takes a while.

The probe exits with an error when a check failed or the script stopped,
after writing everything.

### Example

```text
# A chest: place it, give a stack, open it, move 2 items in, close it.
cmd /block set 525 121 254 Rock_Stone
cmd /block set 525 122 254 Furniture_Crude_Chest_Small
wait block 525 122 254
cmd /give Prober Rock_Stone_Cobble --quantity=5
wait inventory
use 525 122 254
wait window
move hotbar 0 2 window1 3
wait packet 201
expect item window1 3 Rock_Stone_Cobble 2
state moved
close
wait close
```

### Replaying a recording

`replay DIR` sends the interaction chains of a recording (a folder of
client packets with its `index.tsv`, as recorded from a real client) again:

- in order, at the recorded pace (from `index.tsv`);
- moved so that the first block the recording names is `at X Y Z` (by
  default the block under the player);
- with the probe's active hotbar slot;
- with root interaction ids translated by name when `roots FILE` is given:
  `FILE` is the `roots.tsv` of a probe run on the server the recording was
  made on. Two servers number root interactions differently, so a
  recording made on Aerie needs `roots` (an Aerie run's `roots.tsv`) when
  it is replayed on the official server, and the other way round.

The folder must hold each chain from its first packet (the one that starts
it, `initial`): later packets of a chain alone are refused. Give the probe
the item the recording held (`/give`, then `slot`) before replaying.

## Comparing two runs

- `tools/diff/state.py RUN_A RUN_B [--label NAME] [--position] [--skip
  key,key]`: the states, one line per value that differs.
- `tools/diff/cells.py`, `chains.py`, `effects.py`, `stats.py`, `items.py`,
  `falling.py`: the packets of one system (`docs/PROJECT.md`).

Things to know when reading a difference:

- Block changes travel on a `uni` stream and sounds on the game stream:
  compare order only within one stream.
- Two probes at once sometimes fail with "too many gaps in stream buffer":
  run again before believing a difference.
- Chunks around a new position arrive over a second or two after a `goto`:
  `use` and `expect block` wait for the section, other steps do not.
- Each server places the probe at its own spawn: test somewhere both can
  reach, or `goto` the same place on both.
