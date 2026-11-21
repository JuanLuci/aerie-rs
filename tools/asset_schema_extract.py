"""Condenses the official server's asset JSON schemas into gen/assets.json.

The official server writes a JSON schema for every asset type when started
with `--generate-asset-schema <dir>` (it exits right after). Those schemas
describe the asset formats: fields, types, defaults, which fields inherit from
`Parent` (`hytale.inheritsProperty`), references to other assets
(`hytaleAssetRef`) and contained assets (an `anyOf` of a key string and the
referenced asset's own schema, spec 07 "Contained assets").

This keeps only that structure: descriptions, titles, editor hints and
comment fields are dropped.

    python tools/asset_schema_extract.py <dir with the generated Schema/*.json> gen/assets.json
"""

import json
import os
import sys

DROP = {
    "description",
    "markdownDescription",
    "enumDescriptions",
    "markdownEnumDescriptions",
    "title",
    "doNotSuggest",
    "$id",
}


def strip(v):
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            if k in DROP or (k.startswith("$") and k != "$ref"):
                continue
            if k == "hytale" and isinstance(x, dict):
                x = {kk: vv for kk, vv in x.items() if not kk.startswith("ui") and kk != "type"}
                if not x:
                    continue
            out[k] = strip(x)
        return out
    if isinstance(v, list):
        return [strip(x) for x in v]
    return v


def main():
    src, dst = sys.argv[1], sys.argv[2]
    files = {}
    for name in sorted(os.listdir(src)):
        if name.endswith(".json"):
            with open(os.path.join(src, name), encoding="utf-8") as f:
                files[name] = strip(json.load(f))
    with open(dst, "w", encoding="utf-8", newline="\n") as f:
        json.dump({"files": files}, f, separators=(",", ":"))
        f.write("\n")
    print(f"{len(files)} schema files -> {dst}")


if __name__ == "__main__":
    main()
