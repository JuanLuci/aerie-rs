---
name: aerie-probe
description: Compare Aerie with the official server using the probe (a scripted client in aerie-server). Use whenever a task says to compare, check against the official server, or reproduce a behaviour: writing a probe script, running it on both servers, reading state.json and diffing the runs.
---

# Comparing with the probe

A system is done only when Aerie answers like the official server. The
probe is how you show it. The manual is `docs/probe.md`: read its
"Scripts" section before writing one. This file is the way to work with
it.

## 1. Write one script, two places

Write the scenario once with placeholders for its coordinates, then make
one copy per server: each server spawns the probe somewhere else, and you
test only in your own area (your brief gives it). Typical shape:

```text
goto X+0.5 Y Z+0.5          # Aerie copy only, when your area is not at the spawn
cmd /block set X Y-1 Z Rock_Stone
cmd /block set X Y Z Furniture_Crude_Chest_Small
wait block X Y Z
cmd /give Prober ITEM --quantity=5
wait inventory
state ready
use X Y Z                    # or act / move / replay …
wait window                  # wait for what the action should cause
state after
expect …                     # what you expect on both servers
```

Rules that save time:

- Prepare the ground with `cmd` steps (they wait for the reply) and
  `wait block` / `wait inventory`, never with a fixed `sleep`.
- After every action, `wait` for what it should cause before you look:
  a `state` or `expect` right after an action sees the old state.
- Put a `state LABEL` before and after the interesting step: the labelled
  states are what you diff.
- Build actions from names (`use`, `act`, `move`, `slot`), not raw hex.
  Use `send ID:HEX` only for a packet the script has no step for (and tell
  the coordinating session, so a step can be added).
- A recording: `replay DIR at X Y Z roots ROOTS.tsv`, where `ROOTS.tsv` is
  the `roots.tsv` of a probe run on the server the recording was made on.
  The folder must start each chain at its first packet; `/give` and
  `slot` the item it held first.

## 2. Run both

```text
aerie-server probe --upstream 127.0.0.1:OFFICIAL --capture run-o --name Prober --script s-o.txt > run-o.log 2>&1 &
aerie-server probe --upstream 127.0.0.1:AERIE    --capture run-a --name Prober --script s-a.txt > run-a.log 2>&1; wait
```

- Restart your Aerie (fresh world) before each run, and install the binary
  you just built: a stale world or binary gives differences that are not
  real.
- Exit status 0 means the script finished and every check passed. Otherwise
  read `state.json` → `script`: `error` (the step that could not run) or
  `checks` with `"ok": false` (what was there instead, in `got`).
- "too many gaps in stream buffer", "connection lost": run again. Retry up
  to three times before believing anything.

## 3. Compare

```text
python3 tools/diff/state.py run-o run-a --label after      # the player
python3 tools/diff/cells.py run-o X,Y,Z  > o.txt           # blocks and sounds
python3 tools/diff/cells.py run-a X,Y,Z  > a.txt; diff o.txt a.txt
```

and the `tools/diff` script of your system (`chains.py`, `items.py`,
`stats.py`, `effects.py`, `falling.py`). A difference you cannot tell from
the decoded packets: `dump --capture run-o` prints them all.

Reading differences:

- `position`, `body`, `look` differ when the two runs play in different
  places; `state.py` skips the position unless `--position`.
- `chat`: the official server sends message keys with parameters; a
  difference there is real (Aerie should send the same key).
- Compare packet order only within one stream (blocks on `uni`, sounds on
  `game`).
- A difference outside your system is not yours to fix: write it in the
  pull request under "Left out, and why" and tell the coordinating session.

## 4. Report

In the pull request, under "Checked against the official server": the
script (inline, with its coordinates), the command you ran, and the diff
output (`same`, or each remaining difference with why). Keep scripts and
captures out of the commit unless the brief asks for a script file; never
commit captures.

## When the probe is the problem

A step you need does not exist, the state lacks something you must
compare, or the probe reads a packet wrong: do not work around it in your
system's code. Probe changes go in a pull request of their own (`probe.rs`,
`probe/`, its options in `main.rs`); tell the coordinating session what
you need first.
