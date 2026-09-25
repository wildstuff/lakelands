#!/usr/bin/env python3
"""An independent check of the log-format test vectors.

It shares no code with Lakelands: its own RFC 8785 serialiser (members
sorted by UTF-16 code units, the ECMAScript number form), SHA-256 from
the standard library, and the rules of docs/log-format.md read as text.
Run it from this directory: `python3 verify.py`. It prints one line per
check and exits non-zero on the first mismatch.

Given a directory, `python3 verify.py <dir>` checks a lake's log as an
export or a SQLite lake directory holds it instead, reading only
`<dir>/log/` (log-format §4.2): canonical lines each ending in a line
feed, positions consecutive from 0 and naming their files in 20 digits,
the hash chain from 64 zeros, every checkpoint against the position,
the hash and the files before it, and format versions that are 1 or 2
and never go down. It prints one line
per file and the last position and hash, or the position where the
files stop being the log, and exits non-zero there.
"""

import hashlib
import json
import math
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def es_number(f):
    """A finite double in ECMAScript Number.prototype.toString form."""
    if not math.isfinite(f):
        raise ValueError("canonical JSON has no NaN or infinity")
    if f == 0:
        return "0"
    sign = "-" if f < 0 else ""
    # repr() gives the shortest round-trip digits; rebuild them as
    # digits and a decimal exponent n, the value being 0.digits * 10^n.
    mantissa, _, exp = repr(abs(f)).partition("e")
    whole, _, frac = mantissa.partition(".")
    digits = (whole + frac).lstrip("0")
    n = len(whole) + (int(exp) if exp else 0) - (len(whole + frac) - len((whole + frac).lstrip("0")))
    digits = digits.rstrip("0")
    k = len(digits)
    if k <= n <= 21:
        body = digits + "0" * (n - k)
    elif 0 < n <= 21:
        body = digits[:n] + "." + digits[n:]
    elif -6 < n <= 0:
        body = "0." + "0" * (-n) + digits
    else:
        e = n - 1
        es = ("+" if e >= 0 else "") + str(e)
        body = digits + "e" + es if k == 1 else digits[0] + "." + digits[1:] + "e" + es
    return sign + body


def es_string(s):
    out = ['"']
    short = {'"': '\\"', "\\": "\\\\", "\b": "\\b", "\t": "\\t", "\n": "\\n", "\f": "\\f", "\r": "\\r"}
    for c in s:
        if c in short:
            out.append(short[c])
        elif ord(c) < 0x20:
            out.append("\\u%04x" % ord(c))
        else:
            out.append(c)
    out.append('"')
    return "".join(out)


def jcs(v):
    """RFC 8785 canonical form, as text."""
    if v is None:
        return "null"
    if v is True:
        return "true"
    if v is False:
        return "false"
    if isinstance(v, int):
        if abs(v) > 2**53 - 1:
            raise ValueError("an integer outside 2^53 - 1 has no agreed canonical form")
        return str(v)
    if isinstance(v, float):
        return es_number(v)
    if isinstance(v, str):
        return es_string(v)
    if isinstance(v, list):
        return "[" + ",".join(jcs(x) for x in v) + "]"
    if isinstance(v, dict):
        names = sorted(v, key=lambda k: k.encode("utf-16-be"))
        return "{" + ",".join(es_string(k) + ":" + jcs(v[k]) for k in names) + "}"
    raise TypeError(type(v))


def canonical(v):
    return jcs(v).encode("utf-8")


def sha(b):
    return hashlib.sha256(b).hexdigest()


def content_id(*parts):
    return sha(b"\x00".join(parts))[:32]


def key_component(value):
    """A key field's value as a key-string component (log-format §2.3)."""
    if isinstance(value, bool):
        text = "true" if value else "false"
    elif isinstance(value, (int, float)):
        text = jcs(value)
    else:
        text = value
    return text.replace("%", "%25").replace(":", "%3A")


def check(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        sys.exit(1)


FIRST_FORMAT, NEWEST_FORMAT = 1, 2


def verify_dir(root):
    """Check the segment files under `root/log/` alone; see the module doc."""
    files = sorted(p for p in (root / "log").iterdir() if p.suffix == ".jsonl")
    check(bool(files), f"{root}/log holds segment files")
    position, prev, version, known_at = 0, "0" * 64, 0, ""
    sealed = []

    def need(ok, why):
        if not ok:
            print(f"FAIL position {position}: {why}")
            sys.exit(1)

    for i, path in enumerate(files):
        first = position
        need(len(path.stem) == 20 and path.stem.isdigit() and int(path.stem) == first,
             f"{path.name} is the file starting at {first}, named in 20 digits")
        raw = path.read_bytes()
        need(raw.endswith(b"\n"), f"{path.name} ends in a line feed")
        for line in raw[:-1].split(b"\n"):
            try:
                event = json.loads(line.decode("utf-8"))
            except ValueError as e:
                need(False, f"the line is JSON ({e})")
            need(canonical(event) == line, "the line is canonical")
            need(event.get("position") == position, f"the line is position {event.get('position')}")
            v = event.get("v")
            need(type(v) is int and FIRST_FORMAT <= v <= NEWEST_FORMAT,
                 f"format {v} is one of {FIRST_FORMAT} to {NEWEST_FORMAT}")
            need(v >= version, f"format {v} is not below the {version} before it")
            version = v
            # known_at is in its one form (§1.3), so its text orders as its
            # instant does: never before the event before it.
            k = event.get("known_at")
            need(isinstance(k, str) and k >= known_at,
                 f"known_at {k} is not before the {known_at} before it")
            known_at = k
            need(event.get("prev") == prev, "prev names the hash of the event before it")
            body = {k: x for k, x in event.items() if k != "hash"}
            need(sha(canonical(body)) == event.get("hash"), "the hash is SHA-256 of the line without it")
            is_checkpoint = event["body"].get("kind") == "checkpoint"
            if position == first and i > 0:
                need(is_checkpoint, "a later file starts with a checkpoint")
            if is_checkpoint:
                b = event["body"]
                need(position == first and i > 0, "a checkpoint is the first event of a later file")
                need(b.get("through") == position - 1, "the checkpoint names the position before it")
                need(b.get("hash") == prev, "the checkpoint repeats the sealed segment's last hash")
                need(b.get("segments") == sealed, "the checkpoint lists every file before it with its digest")
            prev = event["hash"]
            position += 1
        need(position > first, f"{path.name} holds an event")
        sealed.append({"first": first, "last": position - 1, "digest": sha(raw)})
        print(f"ok   {path.name}: positions {first}..{position - 1}")
    print(f"ok   {root}: verified through position {position - 1}, hash {prev}")


def main():
    if len(sys.argv) > 1:
        return verify_dir(Path(sys.argv[1]))
    expected = json.loads((HERE / "expected.json").read_text("utf-8"))
    prev = "0" * 64
    events = []
    key_fields = {}
    for seg in expected["segments"]:
        raw = (HERE / seg["file"]).read_bytes()
        check(sha(raw) == seg["digest"], f"{seg['file']}: digest {seg['digest'][:12]}…")
        lines = raw.split(b"\n")
        check(lines[-1] == b"", f"{seg['file']}: every event ends with one line feed")
        for line in lines[:-1]:
            event = json.loads(line.decode("utf-8"))
            check(canonical(event) == line, f"position {event['position']}: the stored line is canonical")
            events.append(event)
        check(
            [e["position"] for e in events[-(len(lines) - 1):]] == list(range(seg["first"], seg["last"] + 1)),
            f"{seg['file']}: positions {seg['first']}..{seg['last']}",
        )

    for event, want in zip(events, expected["events"], strict=True):
        pos = event["position"]
        body = {k: v for k, v in event.items() if k != "hash"}
        check(event["prev"] == prev, f"position {pos}: prev names the previous hash")
        check(sha(canonical(body)) == event["hash"] == want["hash"], f"position {pos}: hash {want['hash'][:12]}…")
        if want["id_from"] == "ingest":
            b = event["body"]
            version_before = str(b["version"] - 1).encode()
            got = content_id(b"lakelands:ingest:v1", event["source"].encode(), b["type"].encode(),
                             b["key"].encode(), version_before, canonical(b["patch"]))
            check(got == event["id"] == want["id"], f"position {pos}: ingest id {got}")
        elif want["id_from"] == "authored":
            got = content_id(b"lakelands:authored:v1", event["writer"].encode(), event["idempotency_key"].encode())
            check(got == event["id"] == want["id"], f"position {pos}: authored id {got}")
        b = event["body"]
        if b["kind"] == "declared" and b["declaration"]["declare"] == "type":
            key_fields[b["declaration"]["name"]] = b["declaration"].get("key", [])
        if b["kind"] in ("record_upserted", "record_ingested"):
            fields = key_fields[b["type"]]
            if fields and all(f in b["patch"] for f in fields):
                key = ":".join(key_component(b["patch"][f]) for f in fields)
                check(key == b["key"], f"position {pos}: key {key} from its key fields")
        if event["body"]["kind"] == "checkpoint":
            b = event["body"]
            check(b["hash"] == event["prev"], f"position {pos}: the checkpoint repeats the sealed segment's last hash")
            listed = [{"first": s["first"], "last": s["last"], "digest": s["digest"]} for s in expected["segments"][:-1]]
            check(b["segments"] == listed, f"position {pos}: the checkpoint lists every sealed segment")
        prev = event["hash"]

    view = json.loads((HERE / "schema-hash.json").read_text("utf-8"))
    shape = canonical(view["shape"])
    check(shape.decode("utf-8") == view["shape_canonical"], "schema-hash.json: the shape object's canonical bytes")
    check(sha(shape) == view["schema_hash"], f"schema-hash.json: schema hash {view['schema_hash'][:12]}…")

    v2 = json.loads((HERE / "rekey.json").read_text("utf-8"))
    event = json.loads(v2["line"])
    check(canonical(event) == v2["line"].encode("utf-8"), "rekey.json: the version-2 line is canonical")
    body = {k: v for k, v in event.items() if k != "hash"}
    check(event["v"] == 2 and event["body"]["kind"] == "migrated" and event["body"]["rekeyed"],
          "rekey.json: a version-2 `migrated` event with `rekeyed`")
    check(sha(canonical(body)) == event["hash"] == v2["hash"], f"rekey.json: hash {v2['hash'][:12]}…")
    check(event["prev"] == expected["events"][-1]["hash"], "rekey.json: it follows position 7")

    for n, want in [(1e21, "1e+21"), (1e-7, "1e-7"), (0.000001, "0.000001"), (333333333.3333333, "333333333.3333333"),
                    (-0.0, "0"), (9007199254740992.0, "9007199254740992"), (5e-324, "5e-324")]:
        check(es_number(n) == want, f"number form {want}")


if __name__ == "__main__":
    main()
