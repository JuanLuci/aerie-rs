# 14 — Inventory moves

Behaviour of the official server, written from reading it and checked
with captures; no code.

## Sections and filters

The player's sections (spec 05 ids): hotbar, storage, armor (4), utility
(4), tools, backpack, abilities (6), rune bag. An open structural bench
window adds its own section under the window id: slot 0 the input, 1–64
the options.

What a slot takes (adding):

- armor slot *i*: only armor whose armor slot is *i* (head, chest, hands,
  legs);
- utility: only usable utility items;
- rune bag: only items with an ability;
- abilities, in lines of three: the first of a line takes a core ability
  not already in another line's first slot; the others take a support
  ability that suits the line's core (its required-any, required-all and
  excluded keywords against the core's keywords) and is not already on the
  line;
- window input: only an item some option can be made from; window options
  take and give nothing.

## Moving a stack onto a slot (`MoveItemStack`)

The quantity comes off the slot, all of it or nothing (more than the slot
holds: nothing; a quantity of 0 is a protocol error for the official
server). If the target refuses it, it goes back. Onto an empty slot it is
set; onto a stack it stacks with (same item, durability, max) they add up
to the item's max stack, the rest back on the source; onto anything else
the two swap when the whole source left and the source slot takes the
other stack, else nothing moves. Moving a slot onto itself therefore ends
as it was. Moving into the utility section from another section makes the
target slot the active utility slot, and the client gets `SetActiveSlot`.

## Adding into several sections

Adding a stack into an ordered list of sections fills stacks it stacks with
first (in order, each up to the max, through the filters), then empty
slots. Moving a stack from a slot into such a list takes the quantity off
(all or nothing) and puts back what did not fit.

## `SmartMoveItemStack`

- 0, equip or merge (double click): armor goes onto its armor slot
  (whatever is there); otherwise stacks it stacks with are pulled into the
  slot, first from the open window's slots, then from armor, hotbar,
  utility, storage, backpack in that order: smallest stacks first (then by
  place), until the slot is full.
- 1, put in the hotbar or the window (shift click): from a window slot,
  into the player's pickup sections; else into the open window's slots
  first (done if any of it went in); then armor onto its armor slot if
  that is empty; then storage → hotbar, hotbar → storage, other sections →
  the pickup sections.
- 2, put in the hotbar or the backpack: from the backpack into the pickup
  sections, else into backpack, storage, hotbar.

The pickup sections come from the player's `SyncPlayerPreferences`, per
kind of item (armor; weapons and tools; usable items; items that place a
solid block; the rest): hotbar → hotbar, storage, backpack; storage →
storage, hotbar, backpack; backpack → backpack, storage, hotbar (hotbar by
default). Picked-up items go there too.

## Other packets

`DropItemStack` drops the quantity only if the slot holds that many.
`SetActiveSlot` sets the utility or tools slot in use; the official server
disconnects a client asking for the hotbar, a slot out of range or the
slot already active (Aerie ignores the request instead).

## What clients receive

A section's stacks are listed in the order of the official server's map
(the open addressing table of spec 10's sets, made for 16 entries and
filled in ascending slot order: slots 0–5 come as 0, 2, 4, 1, 3, 5).
Inventory packets from client moves go out before the window's update.

Every section a successful move set is sent at the end of the tick, even
when its content did not change (a slot moved onto itself, a stack that
went back where it was).

## Not done yet

Moving onto or off the active hotbar slot with a different item runs the
swap interaction first (the move is the same); core abilities leaving
their slot take their line's supports along; `InventoryAction` (sort, take
all, put all, quick stack).

## Checked against the official server

The same moves sent to both servers (stacks split into storage, moved onto
themselves, double clicks, shift clicks both ways, a move to the backpack)
give the same inventories and the same inventory packets.

## Kept between sessions

A player's inventory is kept with the world, and with it the active
hotbar, utility and tools slots: a player who comes back holds what was
held when leaving, and the server names those slots right after the
inventory at join. Aerie writes both when the player leaves and, when
they changed, with the periodic save (every 150 ticks), so a server that
stops without warning loses a few seconds at most.
