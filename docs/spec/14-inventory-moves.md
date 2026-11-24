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

## Inventory actions (`InventoryAction`)

One packet: a section id, an action (take all, put all, quick stack,
sort) and a data byte the official server ignores. Only section ids of 0
or more (an open window's id; 0 is the storage, and only for sort) or the
backpack are looked at; the others do nothing. A window action does
nothing unless that window is open and holds items (a container, a
processing bench or a structural bench).

- **Take all**: every stack of the section, slot by slot from the first,
  goes to the pickup sections of its kind (hotbar, storage, backpack in the
  order the player's preferences give), as a move of the whole quantity:
  onto stacks it stacks with first, then empty slots; what does not fit
  stays where it was. A processing bench only gives its output slots (not
  fuel or input); other windows give every slot that may give.
- **Put all**: every stack of the storage (not the hotbar), slot by slot,
  goes into the section, with the section's filters; what does not fit
  stays.
- **Quick stack**: every stack of the hotbar, then of the storage, slot by
  slot, goes into the section, but only a stack of a kind the section
  already holds (the check is made slot by slot, as the section fills);
  it goes onto stacks it stacks with first, then into empty slots of the
  section.
- **Sort** (storage, backpack or an open window's section): stacks of the
  same kind are first merged, in slot order, each up to its max stack
  (items with a max of 1 never merge); the stacks are then ordered by
  kind (weapon, armor, tool, plain item, builder tool), then by the item's
  name key compared as text (character code units); stacks tied keep
  their order. They are laid out from the first slot; slots that cannot
  give their stack, or cannot take the next one, are skipped. Any
  non-empty section is sent afterwards, even when the order did not
  change.

The answers are the ones of a move: the changed sections of
`UpdatePlayerInventory` at the end of the tick, then the window's update.

## Not done yet

Moving onto or off the active hotbar slot with a different item runs the
swap interaction first (the move is the same); core abilities leaving
their slot take their line's supports along; the three window actions
(take all, put all, quick stack) are not compared with the official
server (see below).

## Checked against the official server

The same moves sent to both servers (stacks split into storage, moved onto
themselves, double clicks, shift clicks both ways, a move to the backpack)
give the same inventories and the same inventory packets.

Inventory actions without a window, on both servers with the same items
and the same packets: sorting the storage (merging, kinds, name order,
an empty storage, a storage whose order does not change: the packet is
sent each time), and the backpack, and the ignored cases (a window that is
not open, the hotbar or storage as the section of a take, put or stack,
other action types) give the same
inventory packets. Opening a container with the probe did not work on
the official server (its use chain failed), so take all, put all, quick
stack and a window sort were checked by unit tests only.

## Kept between sessions

A player's inventory is kept with the world, and with it the active
hotbar, utility and tools slots: a player who comes back holds what was
held when leaving, and the server names those slots right after the
inventory at join. Aerie writes both when the player leaves and, when
they changed, with the periodic save (every 150 ticks), so a server that
stops without warning loses a few seconds at most.
