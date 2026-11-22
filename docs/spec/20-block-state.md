# 20. State that lives with a block

Some blocks carry state of their own: what a processing bench holds and
how far it is, a chest's items, a bench's tier, who placed a block. On the
official server this state is a block entity stored with the block's
chunk; it is created with the block, dropped (with its contents, spec 16)
when the block goes, kept when the block only changes its interaction
state, and saved and loaded with the chunk.

## A processing bench's state

Saved: the input, fuel and output slots; the progress of the current
recipe; the fuel time left; whether it is switched on; the count to the
next extra output; the current recipe; and the game time of its last
tick. Not saved: what its windows show (rebuilt on opening).

When a bench comes back (its chunk loads) and it has a last tick time,
the game time that passed since is turned into real seconds (by the
length of the day) and, capped at one day, processed in one go: a furnace
left in an unloaded chunk catches up. Game time does not run while the
server is stopped, so a restart alone adds nothing.

## In Aerie

The world is kept in memory, so blocks never unload: there is nothing to
catch up yet. Bench state is written to `block-state.txt` next to the
changed blocks, whole, every 150 ticks when it changed (`@ x y z kind`,
the state's lines, an empty line), and read at start; a bench gets its
saved state back at its first tick, when its block is still a processing
bench.

## Not done yet

Chests and other containers; bench tiers and upgrade items; who placed a
block; state carried by an item and put back with the block; catching up
after an unloaded time (needs chunk unloading and running game time);
saving on shutdown (up to 5 s of bench progress can be lost).

## Checked against the official server

A furnace loaded with ore and wood on both servers and left to finish,
both servers restarted: opened again it shows the same slots, progress
and switch state (the fuel time differs in the sixth digit, from the two
servers' tick lengths).
