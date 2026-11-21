"""Extracts wire-format facts from the decompiled protocol classes into a JSON schema.

For every type: kind (struct / enum / poly), and for structs the layout:
nullable-bit bytes, fixed fields with their offset and encoding, packed bools,
and the variable fields in wire order. Element encodings come from the Java
field types. Only facts are recorded (names, types, offsets, bits); no code.

usage: python -I schema_extract.py <decompiled protocol package dir> <out.json>

The decompiled sources come from your own copy of the server; they are never
kept in this repository.
"""
import json
import math
import os
import re
import sys

SRC = sys.argv[1]
OUT = sys.argv[2]

PRIM = {"boolean": "bool", "byte": "i8", "short": "i16", "int": "i32", "long": "i64", "float": "f32", "double": "f64",
        "Boolean": "bool", "Byte": "i8", "Short": "i16", "Integer": "i32", "Long": "i64", "Float": "f32", "Double": "f64"}
PROTO_W = {"BOOL": "bool", "BYTE": "i8", "SHORT": "i16", "INT": "i32", "LONG": "i64", "FLOAT": "f32", "DOUBLE": "f64"}
FIXED_HELPERS = {"UUID": "uuid", "Vector2f": "vec2f", "Vector3f": "vec3f", "Quaternionf": "quatf"}


def files():
    for root, _, names in os.walk(SRC):
        if os.sep + "io" in root[len(SRC):] or "/io" in root[len(SRC):].replace("\\", "/"):
            continue
        for n in names:
            if n.endswith(".java"):
                yield os.path.join(root, n)


def method_body(text, signature_regex):
    m = re.search(signature_regex, text)
    if not m:
        return None
    i = text.index("{", m.end() - 1)
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[i + 1:j]
    return None


def parse_type(t):
    """Java type string -> type tree."""
    t = t.strip()
    if t.endswith("[]"):
        return {"array": parse_type(t[:-2])}
    m = re.match(r"^Map<(.+)>$", t)
    if m:
        inner = m.group(1)
        depth = 0
        for k, ch in enumerate(inner):
            if ch == "<":
                depth += 1
            elif ch == ">":
                depth -= 1
            elif ch == "," and depth == 0:
                return {"map": [parse_type(inner[:k]), parse_type(inner[k + 1:])]}
    if t in PRIM:
        return {"prim": PRIM[t], "boxed": t[0].isupper()}
    if t == "String":
        return {"string": True}
    if t == "UUID":
        return {"uuid": True}
    if t in ("Vector2f", "Vector3f", "Quaternionf", "Vector2fc", "Vector3fc", "Quaternionfc"):
        return {"fixed_helper": FIXED_HELPERS[t.rstrip("c")]}
    return {"ref": t}


FIELD_RE = re.compile(r"^   public (?!static)([^(=;]+?) (\w+)(?: = [^;]*)?;$", re.M)
OWN = {}  # class -> (own field decls, superclass)


def preload():
    for p in files():
        text = open(p, encoding="utf-8").read().replace("\r", "")
        m = re.search(r"^public (?:abstract )?class (\w+)(?: extends (\w+))?", text, re.M)
        if not m:
            continue
        own = [(re.sub(r"@\w+ ", "", t), f) for t, f in FIELD_RE.findall(text)]
        OWN[m.group(1)] = (own, m.group(2))


def all_fields(name):
    """Fields of a class including the ones it inherits, base class first."""
    own, sup = OWN.get(name, ([], None))
    return (all_fields(sup) if sup else []) + own


def parse_class(path):
    text = open(path, encoding="utf-8").read().replace("\r", "")
    m = re.search(r"^public (abstract )?(class|enum) (\w+)", text, re.M)
    if not m:
        return None
    abstract, kind, name = m.group(1), m.group(2), m.group(3)

    if kind == "enum":
        values = [(v, int(n)) for v, n in re.findall(r"^   (\w+)\((-?\d+)\)[,;]", text, re.M)]
        return name, {"kind": "enum", "values": values}

    if abstract:
        cases = re.findall(r"case (\d+) -> (\w+)\.toObject", text)
        if cases:
            return name, {"kind": "poly", "variants": [[int(n), s] for n, s in cases]}
        return None

    consts = {k: int(v) for k, v in re.findall(r"public static final int (\w+) = (-?\d+);", text)}
    decl = {}
    order = []
    for t, f in all_fields(name):
        decl[f] = parse_type(t)
        order.append(f)
    body = method_body(text, r"public int serialize\(@Nonnull MemorySegment mem, int offset\) \{")
    if body is None:
        return name, {"kind": "opaque"}
    lines = [l.strip() for l in body.split("\n") if l.strip()]

    null_bits, pending, last_null = {}, [], None
    bools, pend_bool, last_bool = {}, [], None
    fixed, var_fields = [], []
    var_start = consts.get("VARIABLE_BLOCK_START")
    slot = None
    expect_var = None  # slot seen, waiting for the field it belongs to
    enum_int = set()

    for idx, l in enumerate(lines):
        if m := re.match(r"if \(this\.(\w+) != null\) \{", l):
            last_null = m.group(1)
        if m := re.match(r"nullBits = \(byte\)\(nullBits \| (\d+)\);", l):
            pending.append((last_null, int(m.group(1))))
            continue
        if m := re.match(r"mem\.set\(PacketIO\.PROTO_BYTE, offset \+ (\d+), nullBits\);", l):
            for f, n in pending:
                null_bits[f] = 8 * int(m.group(1)) + int(math.log2(n))
            pending = []
            continue
        if m := re.match(r"if \(this\.(\w+)\) \{", l):
            last_bool = m.group(1)
        if m := re.match(r"(boolBits\w+) = \(byte\)\(\1 \| (\d+)\);", l):
            pend_bool.append((last_bool, int(m.group(2)), m.group(1)))
            continue
        if m := re.match(r"mem\.set\(PacketIO\.PROTO_BYTE, offset \+ (\d+) \+ (\d+), (boolBits\w+)\);", l):
            at = int(m.group(1)) + int(m.group(2))
            for f, n, var in pend_bool:
                if var == m.group(3):
                    bools[f] = [at, int(math.log2(n))]
            pend_bool = [p for p in pend_bool if p[2] != m.group(3)]
            fixed.append({"packed_bools": at})
            continue
        if "PROTO_INT" in l and "getValue()" in l and "this." in l:
            mm = re.search(r"this\.(\w+)", l)
            enum_int.add(mm.group(1))
        # variable-field slot
        if m := re.match(r"mem\.set\(PacketIO\.PROTO_INT, offset \+ (\d+), varOffset - offset - (\d+)\);", l):
            slot = int(m.group(1))
            expect_var = slot
            continue
        if m := re.match(r"int varOffset = offset \+ (\d+);", l):
            var_start = int(m.group(1))
            continue
        # fixed fields
        if m := re.match(r"mem\.set\(PacketIO\.PROTO_(\w+), offset \+ (\d+), (?:\((\w+)\))?this\.(\w+)(\.getValue\(\))?\);", l):
            proto, off, f = m.group(1), int(m.group(2)), m.group(4)
            fixed.append({"field": f, "at": off, "enc": {"enum_width": PROTO_W[proto], "ref": decl[f].get("ref")} if m.group(5) else {"prim": PROTO_W[proto]}})
            continue
        if m := re.match(r"this\.(\w+)\.serialize\(mem, offset \+ (\d+)\);", l):
            fixed.append({"field": m.group(1), "at": int(m.group(2)), "enc": {"struct": decl[m.group(1)]["ref"]}})
            continue
        if m := re.match(r"PacketIO\.write(UUID|Vector2f|Vector3f|Quaternionf)\(mem, offset \+ (\d+), this\.(\w+)\);", l):
            fixed.append({"field": m.group(3), "at": int(m.group(2)), "enc": {"fixed_helper": FIXED_HELPERS[m.group(1)]}})
            continue
        if m := re.match(r"PacketIO\.writeFixed(Ascii)?String\(mem, offset \+ (\d+), this\.(\w+), (\d+)\);", l):
            fixed.append({"field": m.group(3), "at": int(m.group(2)), "enc": {"fixed_string": int(m.group(4))}})
            continue
        if m := re.match(r"MemorySegment\.copy\(this\.(\w+), 0, mem, PacketIO\.PROTO_(\w+), offset \+ (\d+), this\.\w+\.length\);", l):
            fixed.append({"field": m.group(1), "at": int(m.group(3)), "enc": {"fixed_prim_array": PROTO_W[m.group(2)]}})
            continue
        if m := re.search(r"offset \+ (\d+) \+ \w+ \* (\d+), \(?(?:\((\w+)\))?this\.(\w+)\[\w+\]", l):
            f = m.group(4)
            fixed.append({"field": f, "at": int(m.group(1)), "enc": {"fixed_array": decl[f], "stride": int(m.group(2))}, "line": l})
            continue
        # the field a variable slot belongs to: first this.F mentioned after the slot
        if expect_var is not None:
            if m := re.search(r"this\.(\w+)", l):
                var_fields.append({"field": m.group(1), "slot": expect_var})
                expect_var = None
                continue

    # single variable field: no slot statements
    if consts.get("VARIABLE_FIELD_COUNT", 0) == 1 and not var_fields:
        fixed_names = {x.get("field") for x in fixed}
        after = body.split("int varOffset", 1)[1] if "int varOffset" in body else ""
        for f in re.findall(r"this\.(\w+)", after):
            if f not in fixed_names and f in decl:
                var_fields.append({"field": f, "slot": None})
                break

    # fields not seen anywhere in serialize (constant-size padding etc.) are reported
    placed = {x.get("field") for x in fixed} | {v["field"] for v in var_fields} | set(bools)
    missing = [f for f in order if f not in placed]
    for v in var_fields:
        v["type"] = decl[v["field"]]
        # arrays whose elements may be absent: count, presence bits, present elements
        if re.search(r"int \w+BitfieldSize = \(this\.%s\.length \+ 7\) / 8" % v["field"], body):
            v["type"] = dict(v["type"], nullable_elements=True)
        if v["field"] in enum_int:
            v["enum_int"] = True
    for x in fixed:
        if "field" in x and x["field"] in null_bits:
            x["null_bit"] = null_bits[x["field"]]
    # fixed primitive arrays run to the next fixed field or the end of the fixed block
    width = {"bool": 1, "i8": 1, "i16": 2, "i32": 4, "i64": 8, "f32": 4, "f64": 8}
    for x in fixed:
        if "fixed_prim_array" in x.get("enc", {}):
            later = [y["at"] for y in fixed if y.get("at", -1) > x["at"]]
            end = min(later) if later else consts.get("FIXED_BLOCK_SIZE")
            x["enc"]["count"] = (end - x["at"]) // width[x["enc"]["fixed_prim_array"]]
    for v in var_fields:
        if v["field"] in null_bits:
            v["null_bit"] = null_bits[v["field"]]
    return name, {
        "kind": "struct",
        "null_bytes": consts.get("NULLABLE_BIT_FIELD_SIZE", 0),
        "fixed_size": consts.get("FIXED_BLOCK_SIZE"),
        "var_count": consts.get("VARIABLE_FIELD_COUNT", 0),
        "var_start": var_start,
        "max_size": consts.get("MAX_SIZE"),
        "fixed": fixed,
        "bools": bools,
        "var": var_fields,
        "decl_order": order,
        "decl": decl,
        "missing": missing,
        "enum_int_fields": sorted(enum_int),
    }


preload()
schema = {}
for p in files():
    r = parse_class(p)
    if r:
        schema[r[0]] = r[1]
json.dump(schema, open(OUT, "w"), indent=1, sort_keys=True)

kinds = {}
for v in schema.values():
    kinds[v["kind"]] = kinds.get(v["kind"], 0) + 1
print(len(schema), "types", kinds)
bad = {k: v["missing"] for k, v in schema.items() if v["kind"] == "struct" and v["missing"]}
print("structs with unplaced fields:", len(bad))
for k in list(bad)[:25]:
    print(" ", k, bad[k])
cnt = sum(1 for v in schema.values() if v["kind"] == "struct" and v["var_count"] != len(v["var"]))
print("structs whose var count disagrees:", cnt)
for k, v in schema.items():
    if v["kind"] == "struct" and v["var_count"] != len(v["var"]):
        print("  ", k, v["var_count"], [x["field"] for x in v["var"]])
        cnt -= 1
        if cnt < -15:
            break
