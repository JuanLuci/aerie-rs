"""Generates the protocol and asset tables Aerie is built from, on your machine,
from your own copy of the game's server.

The tables describe the wire format and the asset formats (spec 06, spec 07).
They are facts read from the server you installed. They are not kept in this
repository: every builder generates them once, before `cargo build`.

    python tools/gen.py [--server-jar PATH] [--assets PATH] [--java PATH] [--out DIR]

Writes into DIR (default `gen/` at the repository root, which git ignores):

- `packets.tsv`: the packet table (id, name, direction, channel, sizes);
- `protocol.json`: every protocol type with its wire layout
  (`tools/schema_extract.py`);
- `assets.json`: the asset formats, condensed from the schemas the server
  writes with `--generate-asset-schema` (`tools/asset_schema_extract.py`).

What it needs:

- the server jar (`Server/HytaleServer.jar` of an installed game, or of the
  official server download) and `Assets.zip`; found in the default install
  folder, or pass `--server-jar` and `--assets`;
- Java 25 or newer (the game's bundled runtime is used when found);
- Vineflower, a decompiler, to read the protocol classes. It is downloaded
  once from Maven Central into `<out>/.tools` and checked against a pinned
  SHA-256. The decompiled classes stay in a temporary folder and are deleted
  when this script ends; only the tables are kept.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent

VINEFLOWER = "1.12.0"
VINEFLOWER_URL = (
    "https://repo1.maven.org/maven2/org/vineflower/vineflower/"
    f"{VINEFLOWER}/vineflower-{VINEFLOWER}.jar"
)
VINEFLOWER_SHA256 = "1dfcfe974395734fa467ce620661c7623d05ba83670de0529b1fbd63ff548b9d"

PROTOCOL_PKG = "com/hypixel/hytale/protocol/"


def install_dirs():
    """Folders where the game's launcher puts the current release."""
    home = Path.home()
    out = []
    if os.name == "nt":
        appdata = os.environ.get("APPDATA")
        if appdata:
            out.append(Path(appdata) / "Hytale")
    elif platform.system() == "Darwin":
        out.append(home / "Library" / "Application Support" / "Hytale")
    else:
        xdg = os.environ.get("XDG_DATA_HOME")
        out.append(Path(xdg) / "Hytale" if xdg else home / ".local" / "share" / "Hytale")
    return [d / "install" / "release" / "package" for d in out]


def find_server_jar(arg):
    for c in [arg, os.environ.get("AERIE_SERVER_JAR")]:
        if c:
            p = Path(c)
            if p.is_dir():
                p = p / "Server" / "HytaleServer.jar" if (p / "Server").is_dir() else p / "HytaleServer.jar"
            if p.is_file():
                return p
            sys.exit(f"server jar not found: {p}")
    for d in install_dirs():
        p = d / "game" / "latest" / "Server" / "HytaleServer.jar"
        if p.is_file():
            return p
    sys.exit("server jar not found: pass --server-jar (Server/HytaleServer.jar of your game or server download)")


def find_assets(arg, jar):
    for c in [arg, os.environ.get("AERIE_ASSETS")]:
        if c:
            p = Path(c)
            if p.is_file():
                return p
            sys.exit(f"Assets.zip not found: {p}")
    for p in [jar.parent.parent / "Assets.zip", jar.parent / "Assets.zip"]:
        if p.is_file():
            return p
    sys.exit("Assets.zip not found: pass --assets")


def java_major(java):
    try:
        r = subprocess.run([java, "-version"], capture_output=True, text=True)
    except OSError:
        return None
    m = re.search(r'version "(\d+)', r.stderr + r.stdout)
    return int(m.group(1)) if m else None


def find_java(arg, jar):
    exe = "java.exe" if os.name == "nt" else "java"
    cands = [arg, os.environ.get("AERIE_JAVA")]
    # The game's bundled runtime: package/jre/latest next to package/game/latest.
    cands.append(str(jar.parents[3] / "jre" / "latest" / "bin" / exe) if len(jar.parents) > 3 else None)
    if os.environ.get("JAVA_HOME"):
        cands.append(str(Path(os.environ["JAVA_HOME"]) / "bin" / exe))
    cands.append(shutil.which("java"))
    for c in cands:
        if c and Path(c).is_file() or (c and shutil.which(c)):
            v = java_major(c)
            if v and v >= 25:
                return c
    sys.exit("Java 25 or newer not found: pass --java")


def vineflower(tools):
    jar = tools / f"vineflower-{VINEFLOWER}.jar"
    if not jar.is_file():
        tools.mkdir(parents=True, exist_ok=True)
        print(f"downloading Vineflower {VINEFLOWER} from Maven Central")
        tmp = jar.with_suffix(".part")
        with urllib.request.urlopen(VINEFLOWER_URL) as r, open(tmp, "wb") as f:
            shutil.copyfileobj(r, f)
        tmp.replace(jar)
    digest = hashlib.sha256(jar.read_bytes()).hexdigest()
    if digest != VINEFLOWER_SHA256:
        jar.unlink()
        sys.exit(f"Vineflower checksum mismatch ({digest}); deleted, run again")
    return jar


def decompile_protocol(java, vf, server_jar, work):
    classes = work / "classes"
    src = work / "src"
    with zipfile.ZipFile(server_jar) as z:
        names = [n for n in z.namelist() if n.startswith(PROTOCOL_PKG) and n.endswith(".class")]
        if not names:
            sys.exit(f"no protocol classes in {server_jar}")
        for n in names:
            z.extract(n, classes)
    src.mkdir()
    print(f"decompiling {len(names)} protocol classes")
    r = subprocess.run(
        [java, "-jar", str(vf), "-dgs=1", f"-e={server_jar}", str(classes), str(src)],
        capture_output=True,
        text=True,
    )
    pkg = src / PROTOCOL_PKG
    if not pkg.is_dir():
        # Some versions write the package tree under the input folder's name.
        found = list(src.rglob("PacketRegistry.java"))
        if not found:
            sys.exit("decompiling failed:\n" + (r.stdout + r.stderr)[-2000:])
        pkg = found[0].parent
    return pkg


def packet_table(pkg, out, source_name):
    """The packet registrations in PacketRegistry: id, name, direction,
    channel, fixed size, max size, compressed."""
    text = (pkg / "PacketRegistry.java").read_text(encoding="utf-8")
    body = text[text.index("static {"):] if "static {" in text else text
    rows = []
    for call in re.findall(r"register\(\s*(.*?)\);", body, re.S):
        parts = [p.strip() for p in re.split(r",(?![^(]*\))", " ".join(call.split()))]
        if len(parts) < 9 or "PacketDirection" not in parts[0]:
            continue
        rows.append(
            (
                int(parts[2]),
                parts[3].strip('"'),
                parts[0].split(".")[-1],
                parts[1].split(".")[-1],
                int(parts[5]),
                int(parts[6]),
                parts[7] == "true",
            )
        )
    if not rows:
        sys.exit("no packets found in PacketRegistry")
    rows.sort()
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(f"# Packet ids, generated by tools/gen.py from {source_name}.\n")
        f.write("# dir: direction (ToServer, ToClient, Both). fixed: size of the fixed part in bytes.\n")
        f.write("# max: largest payload accepted. zstd: payload is one Zstandard frame.\n")
        f.write("id\tname\tdir\tchannel\tfixed\tmax\tzstd\n")
        for r in rows:
            f.write("\t".join(map(str, r)) + "\n")
    return len(rows)


def protocol_schema(raw, out, source_name):
    """Keeps the layout facts of schema_extract.py's output; its check fields
    (`missing`: fields it could not place, which must be none) are dropped."""
    types = json.loads(raw.read_text(encoding="utf-8"))
    bad = {k: t["missing"] for k, t in types.items() if t.get("missing")}
    if bad:
        sys.exit(f"protocol fields not placed (decompiler output changed?): {dict(list(bad.items())[:5])}")
    drop = {"missing", "var_count", "enum_int_fields"}
    doc = {
        "about": (
            f"Wire layout of every protocol type, generated by tools/gen.py from {source_name}. "
            "Facts only: names, types, offsets, nullable bits. See docs/spec/06-schema.md."
        ),
        "types": {k: {f: v for f, v in t.items() if f not in drop} for k, t in types.items()},
    }
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(doc, f, indent=1, sort_keys=True)
    print(f"protocol.json: {len(types)} types")


def asset_schemas(java, server_jar, assets, work):
    out = work / "schema"
    run = work / "run"
    run.mkdir()
    print("writing the asset schemas with the server (--generate-asset-schema)")
    r = subprocess.run(
        [
            java,
            "-Xmx2500m",
            "-jar",
            str(server_jar),
            "--assets",
            str(assets),
            "--generate-asset-schema",
            str(out),
        ],
        cwd=run,
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
        timeout=900,
    )
    d = out / "Schema" if (out / "Schema").is_dir() else out
    if not any(d.glob("*.json")):
        sys.exit("the server wrote no schemas:\n" + (r.stdout + r.stderr)[-2000:])
    return d


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--server-jar")
    ap.add_argument("--assets")
    ap.add_argument("--java")
    ap.add_argument("--out", default=str(ROOT / "gen"))
    ap.add_argument("--keep", action="store_true", help="keep the temporary folder (decompiled classes)")
    a = ap.parse_args()

    server_jar = find_server_jar(a.server_jar)
    assets = find_assets(a.assets, server_jar)
    java = find_java(a.java, server_jar)
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    vf = vineflower(out / ".tools")
    print(f"server: {server_jar}\nassets: {assets}\njava:   {java}")

    work = Path(tempfile.mkdtemp(prefix="aerie-gen-"))
    try:
        pkg = decompile_protocol(java, vf, server_jar, work)
        n = packet_table(pkg, out / "packets.tsv", server_jar.name)
        print(f"packets.tsv: {n} packets")
        raw = work / "protocol-raw.json"
        subprocess.run(
            [sys.executable, "-I", str(HERE / "schema_extract.py"), str(pkg), str(raw)],
            check=True,
        )
        protocol_schema(raw, out / "protocol.json", server_jar.name)
        schemas = asset_schemas(java, server_jar, assets, work)
        subprocess.run(
            [sys.executable, "-I", str(HERE / "asset_schema_extract.py"), str(schemas), str(out / "assets.json")],
            check=True,
        )
    finally:
        if a.keep:
            print(f"kept {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)
    print(f"done: {out}")


if __name__ == "__main__":
    main()
