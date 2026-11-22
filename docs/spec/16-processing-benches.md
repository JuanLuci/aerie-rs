# 16 — Processing benches

Behaviour of the official server, written from reading it and checked
with captures; no code. Furnaces, campfires, the salvage bench and the
tannery: benches that hold items and work by themselves over time.

## The bench

From the block's `Bench` (`Type` `Processing`): `Input` slots (each with
an `Icon`, a `ResourceTypeId` or `FilterValidIngredients`), `Fuel` slots
(icon and resource type; none: the bench needs no fuel), `MaxFuel` (-1),
`FuelDropItemId`, `OutputSlotsCount` (1), `AllowNoInputProcessing`,
`ExtraOutput` (`Outputs`, `PerFuelItemsConsumed`, `IgnoredFuelSources`),
`EndSoundEventId`, and per tier extra input and output slots and a
crafting time reduction. Its slots, in the window's order: fuel, input,
output.

What a slot takes: an input slot with a resource type takes items giving
that resource; one that filters valid ingredients takes any input material
of the bench's recipes; fuel slots take items giving their resource type;
output slots take nothing from players. Everything can be taken out.

The bench has a recipe when each input material of one of its recipes can
be taken, in exact amounts, from a slot of its own; among several, the
one with the most materials, the current one kept while it still fits. A
new recipe starts from no progress. Putting items in or taking them out
picks the recipe again.

A bench without fuel slots is always active. One with fuel slots is
switched by its player (`SetActiveAction`): switching on is refused (the
window is sent again) while it has no fuel.

## Each tick

With the time since the bench's last tick (none on its first):

- With a recipe: if one craft's outputs would not fit, or the inputs are
  no longer there (then progress and recipe are dropped), the bench goes
  back to its `default` block state (from `Processing` with the failed
  sound, from `ProcessCompleted` with the end sound) and switches off.
  Otherwise the slots the inputs would come from are the processing slots.
- Without a recipe: a bench without fuel slots only goes back to
  `default` as above. One with fuel slots that does not allow working
  without input goes back to `default` (failed sound) and switches off; one
  that allows it keeps burning.
- Then the work: as many crafts as the time, the inputs, the fuel (time
  left plus every fuel item's `FuelQuality`) and the output room allow, at
  the recipe's time less the tier's reduction; fuel is burnt for the time
  used (the time left first, then whole fuel items, one unit of the slot's
  resource each, adding its `FuelQuality` seconds, never past `MaxFuel`),
  then the inputs are taken and the outputs added; what does not fit is
  thrown out of the bench's front (its centre moved half its depth plus
  0.1 forward; velocity 2 forward with ±1 of spread sideways and along,
  3.25 up). Every `PerFuelItemsConsumed` fuel items burnt add the extra
  outputs (a fuel item whose block is an ignored source does not count).
  A bench burning without a recipe just burns for the time passed.
- A fuel bench that is off or has no fuel time left then goes back to
  `default` (failed sound) and switches off. Otherwise the first filled
  fuel slot is the processing fuel slot, and the window's fuel is the time
  left over the last total (the time right after a fuel item was added,
  rounded up).
- A tick that finished crafts puts the block in `ProcessCompleted` with
  the bench's completed sound; any other working tick in `Processing`.

Sounds play at the block's centre (SFX).

## The window

Opened by using the block (page `Bench`, window type `Processing`, then
the bench's open sound): its data in this order: `type`, `id`, `name`,
`blockItemId`, `tierLevel`, `grantedAugmentTags`, `active`, `progress`,
`fuel` (icon and resource type per fuel slot, when it has any),
`maxFuel`, `fuelTime`, `processingFuelSlots`, `processingSlots` (bit
masks), `input` (icon per slot), `outputSlotsCount`, then the bench
fields of spec 09; its inventory is the bench's slots, its stacks listed
in the official map order (spec 14). `progress` is the running craft's
part done. Changes go out once per tick, after the player's inventory.

Closing it is spec 09's close (block state reset, close sound). Breaking
the block (or replacing it with anything but one of its own states) drops
everything inside at the block's bottom centre (spec 11), with the whole
seconds of fuel left as `FuelDropItemId` items, and closes the windows
open on it (`CloseWindow`).

## Not done yet

Bench contents are not saved across restarts; tier upgrades; chests near
the bench; time passed while the bench was unloaded.

## Checked against the official server

A furnace with 5 logs and 3 copper ore, opened with a recorded use chain
(its root interactions translated, the furnace's root entered), loaded,
switched on and left to finish: the same `OpenWindow`, window updates
(recipe slots, activation, per-tick progress and fuel, outputs with the
charcoal), block states, sounds and final state on both servers, apart
from tick timing. A loaded furnace removed: the same drops and
`CloseWindow`.
