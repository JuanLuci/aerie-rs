# 02 · Encoding

## Frames

Each stream is a sequence of frames:

```
u32 LE   payload length (bytes actually on the wire)
u32 LE   packet id
[length] payload
```

- The receiver rejects a frame whose length is negative or above 1 600 MiB,
  whose id is unknown for that direction, or whose length is above that
  packet's maximum size. Any of these closes the connection.
- **Compression is per packet type, not per frame.** Packets marked as
  compressed in `gen/packets.tsv` (`tools/gen.py`) always carry a single Zstandard
  frame as payload, and the length field is the compressed size. The Zstandard
  frame must declare its content size; the receiver refuses frames that do not,
  or whose content size is above the packet's maximum.
- An empty payload (length 0) is valid and is never compressed.

## Scalars

- Integers and floats are little-endian. `bool` is one byte.
- **UUIDs are the exception:** 16 bytes, most significant half first, each half
  big-endian.
- VarInt: unsigned LEB128 (7 bits per byte, low group first, high bit set on
  every byte but the last), at most 5 bytes. Negative values cannot be encoded.
- Strings with a variable length: VarInt byte count, then the bytes (UTF-8, or
  ASCII where the field says so). Each field has its own byte limit.
- Fixed strings: a fixed number of ASCII bytes, zero-padded.
- Byte arrays: VarInt count, then the bytes.

## Struct layout

Every packet and every nested struct uses the same layout:

```
[nullable bits]   ceil(nullable fields / 8) bytes, omitted when there are none
[fixed fields]    in declaration order, each at a fixed offset
[offset table]    one i32 LE per variable-size field, only when there are 2 or more
[variable block]  the variable-size fields, back to back
```

- A bit in the nullable bitfield says whether that optional field is present.
  Fixed-size optional fields still take their space when absent.
- Offsets in the table are relative to the start of the variable block. An
  absent field has offset −1.
- **The layout must be canonical:** variable fields appear in declaration
  order with no gaps, so each offset equals the end of the previous field.
  The reader checks this and rejects anything else.
- With a single variable-size field there is no offset table; the field starts
  right after the fixed fields.
- Nested structs are written inline in the variable block with the same rules.

Example, `Connect` (id 0):

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | nullable bits: 1 = identity token, 2 = referral data, 4 = referral source |
| 1 | 4 | protocol CRC (i32) |
| 5 | 4 | protocol build number (i32) |
| 9 | 20 | client version (fixed ASCII) |
| 29 | 1 | client type (u8 enum) |
| 30 | 16 | offset table: identity token, language, referral data, referral source |
| 46 | … | variable block |

Variable fields of `Connect`: identity token (UTF-8, ≤ 8192 bytes, optional),
language (ASCII, ≤ 100, required), referral data (bytes, ≤ 4096, optional),
referral source (`HostAddress`, optional).
