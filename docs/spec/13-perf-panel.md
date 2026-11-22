# 13 — Performance panel

Aerie's own tool, not a behaviour of the official server: how heavy the
server runs, for players and for the test logs.

## Numbers

The world tick task times each tick (the hub's tick: support checks,
flushes, block health, items, crafting, inventories). Every second it
takes a sample: the average and longest tick of that second in ms (the
budget is 33.3 ms for 30 Hz), ticks per second, the process's resident
memory, the columns some client has, the entities (players, dropped items,
items flying to a player) and the players.

## Log

Every 10 s, with or without the panel, one line:
`perf: ram <MB> MB, tick avg <ms> ms max <ms> ms (budget 33.3), tps <n>,
columns <n>, entities <n>, players <n>`, over those 10 s (memory: the
highest sample).

## Panel

With `--perf-hud`, every player starts with the panel on; `/perf` turns it
on or off for whoever types it, with or without the flag. It is a custom
HUD (`CustomHud`, key `AeriePerf`), made the way the official server makes
its own HUDs: the first showing clears the key and appends a small inline
UI document (a dark box in the top right corner with four labels: tick,
ticks per second, memory, world counts), then sets each label's `Text`;
each later second only sets the texts. Turning it off sends the key
cleared with no commands. Nothing is sent before the player's position is
known.
