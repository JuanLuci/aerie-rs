# 08 · Block sections in `SetChunk`

Observed in chunks the official server (build 0.7.7) sent to Aerie's probe.
Checked on 7 captured sections: every one decodes to exactly 32768 blocks
and the three arrays end exactly where `data` ends.

## The packet

`SetChunk` (131, Chunks stream) carries one section: 32×32×32 blocks at
section coordinates `x`, `y`, `z` (block `x * 32` and so on).

- `data`: null for an all-air section; otherwise the three arrays below.
- `localLight`: one zero byte in every capture (empty light).
- `globalLight`: null in every capture.

## Block index

Block `(x, y, z)` inside the section (0..32 each) is at
`index = y << 10 | z << 5 | x`.

- `y` is the slowest axis: layer 0 of the lowest section is one block
  (1024 of 1024), the bedrock.
- `x` is the fastest: across the face shared by the neighbouring sections
  (14, 3, 0) and (15, 3, 0), air/solid agree as often as between two
  neighbouring layers inside one section (99.1 % against 99.3 %), and
  less with `x`/`z` swapped (95.9 % against 99.8 %).

## `data`: three paletted arrays

In order: block ids, fillers, rotations. Each array is:

| Bytes | |
| --- | --- |
| 1 | palette type: 0 empty (all zero; nothing follows), 1 half byte, 2 byte |
| 2 | i16 LE: number of entries |
| 7 × n | entries: u8 palette index, i32 LE value, i16 LE number of uses |
| 16384 or 32768 | indices: type 1 two per byte, the first in the high nibble; type 2 one per byte |

- Entries come in no particular order; palette indices need not follow it.
- The numbers of uses add up to 32768.
- Block values are BlockTypes indexes (0 is air). Filler values pack three
  5-bit signed offsets (`1024` is y + 1, `31744` is y − 1). Rotation
  values are small numbers (1 to 9 seen).
- Type 1 holds up to 16 entries, type 2 up to 256. A larger type for more
  values was not seen.

## `SetColumn`

Sent on the Chunks stream before the sections of its column (the client
logs "SetChunk arrived before its SetColumn" otherwise).

- `heightmap`: 1024 i32, index `z * 32 + x`: the y of the highest block
  that counts as ground (plants on top do not; 867 of 1024 columns of one
  capture are simply the highest non-air block).
- `tintmap`: i16 LE number of colours; the colours, 4 bytes RGBA each; i32
  LE bitfield length, always 1280 (1024 columns × 10 bits); the bitfield
  of colour indices. Checked against two captured columns (51 colours, 1490
  bytes; 140 colours, 1846 bytes) and by the client's own error message
  ("tintmap bitfield length … must equal 1280").
