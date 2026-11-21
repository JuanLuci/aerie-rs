# 06 · The protocol schema

`gen/protocol.json` (written by `tools/gen.py` from your own copy of the server) describes the wire layout of every protocol
type, 1020 in all: 832 structs, 175 enums and 13 polymorphic types. Aerie
generates its Rust types and codecs from it at build time
(`crates/aerie-protocol/build_codegen.rs` → `aerie_protocol::proto`).

## What it records

Facts about the wire format only:

- **struct**: number of nullable-bit bytes, size of the fixed part, where the
  variable block starts, the fixed fields (offset, encoding, nullable bit),
  packed booleans (byte and bit), and the variable fields in wire order (offset
  slot, nullable bit, type).
- **enum**: the named values.
- **poly**: the variant types by id. On the wire: varint id, then the variant.

Element types: strings, scalars, UUIDs, float vectors, arrays, maps (kept as
ordered pairs), references to other types.

## Encodings beyond spec 02

- **Packed booleans.** Non-nullable booleans can share one byte of the fixed
  part, one bit each.
- **Enum width.** Enums are one signed byte, except where a field stores them
  as a 4-byte integer (marked `enum_width: i32` or `enum_int`).
- **Arrays with optional elements.** Varint count, then ⌈count / 8⌉ bytes of
  presence bits, then only the present elements.
- **Fixed scalar arrays.** A run of scalars inside the fixed part, with no count
  (for example a 32×32 heightmap of i32).
- **Float vectors** are 2, 3 or 4 consecutive f32; UUIDs are 16 bytes,
  most-significant half first.

## How it was produced and checked

The schema was extracted from the official server's packet classes (build
0.7.7) by [`tools/schema_extract.py`](../../tools/schema_extract.py), which
reads layouts and records them; it copies no code. It was then checked against
traffic: 399 packets captured from the official server by Aerie's probe (all
of setup, 18 MB of registries, and the first 20 s of a world join, chunks
included) decode with the generated code and encode back **byte for byte
identical**.

Types that never appeared in those captures follow the same rules but are not
yet checked against real traffic.

To check again after a game update:

```
aerie-server probe --upstream 127.0.0.1:5521 --capture <dir>
cargo run --release -p aerie-protocol --example roundtrip -- <dir>
```
