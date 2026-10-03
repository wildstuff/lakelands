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
the hash and the files before it, format versions from 1 to 10 that
never go down, a verb on its type (declared with `scope: type`, called or
proposed with no `key`) only from version 8 on, a derived relation only from version 6 on, a declaration's `annotations` only from version 5 on, a proposed declaration or migration only from version 4 on, `upstream` only from version 3 on and only on a
`record_ingested` event, and every `known_at` in its one form and never before
the `known_at` before it (log-format §1.3). It prints one line
per file and the last position and hash, or the position where the
files stop being the log, and exits non-zero there.
"""

import hashlib
import json
import math
import re
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


FIRST_FORMAT, NEWEST_FORMAT = 1, 10
ONE_FORM = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{6}Z")


def derives(body):
    """Whether a declaration the event body carries holds what version 6
    added: a derived relation, or a trigger's `max_depth`."""
    decls = []
    kind = body.get("kind")
    if kind == "declared":
        decls = [body["declaration"]]
    elif kind == "migrated":
        decls = [body["migration"]["to"]]
    elif kind == "write_proposed":
        w = body.get("write", {})
        if w.get("write") == "declare":
            decls = w["declarations"]
        elif w.get("write") == "migrate":
            decls = [w["migration"]["to"]]
    return any("derived" in r for d in decls for r in d.get("relations", [])) or any(
        d.get("declare") == "trigger" and "max_depth" in d for d in decls)


def on_type(body):
    """Whether the event body is a verb on its type, which version 8
    added: declared with `scope: type` (in a `declared` event or a proposed
    `declare`), or called (`verb_called`) or proposed (a `call`) without a
    `key`."""
    kind = body.get("kind")
    if kind == "declared":
        decls = [body["declaration"]]
    elif kind == "write_proposed" and body.get("write", {}).get("write") == "declare":
        decls = body["write"]["declarations"]
    else:
        decls = []
    if any(d.get("declare") == "verb" and d.get("scope") == "type" for d in decls):
        return True
    if kind == "verb_called":
        return "key" not in body
    if kind == "write_proposed" and body.get("write", {}).get("write") == "call":
        return "key" not in body["write"]
    return False


def annotated(body):
    """Whether a declaration the event body carries holds a non-empty
    `annotations`: on a type, its fields, its relations and their link
    fields, a verb's payload fields or a function's inputs."""
    def decl(d):
        maps = [d.get("annotations")]
        for f in d.get("fields", []) + d.get("payload", []) + d.get("inputs", []):
            maps.append(f.get("annotations"))
        for r in d.get("relations", []):
            maps.append(r.get("annotations"))
            maps.extend(f.get("annotations") for f in r.get("fields", []))
        return any(maps)
    kind = body.get("kind")
    if kind == "declared":
        return decl(body["declaration"])
    if kind == "migrated":
        return decl(body["migration"]["to"])
    if kind == "write_proposed":
        w = body.get("write", {})
        if w.get("write") == "declare":
            return any(decl(d) for d in w["declarations"])
        if w.get("write") == "migrate":
            return decl(w["migration"]["to"])
    return False


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
            need("upstream" not in event or v >= 3, f"a format {v} event carries `upstream`, which format 3 added")
            proposed = event["body"].get("write", {}).get("write") if event["body"].get("kind") == "write_proposed" else None
            need(proposed not in ("declare", "migrate") or v >= 4,
                 f"a format {v} event proposes a declaration or a migration, which format 4 added")
            need(not on_type(event["body"]) or v >= 8,
                 f"a format {v} event declares or calls a verb on its type, which format 8 added")
            need(not derives(event["body"]) or v >= 6,
                 f"a format {v} event declares a derived relation or a trigger's `max_depth`, which format 6 added")
            need(not annotated(event["body"]) or v >= 5,
                 f"a format {v} event carries `annotations` in a declaration, which format 5 added")
            need("cause" not in event.get("context", {}),
                 "`context` carries `cause`, which the envelope carries")
            need("upstream" not in event or event["body"].get("kind") == "record_ingested",
                 f"a `{event['body'].get('kind')}` event carries `upstream`, which only a `record_ingested` event carries")
            # known_at in its one form (§1.3), whose text orders as its
            # instant does, and never before the event before it.
            k = event.get("known_at")
            need(isinstance(k, str) and ONE_FORM.fullmatch(k) is not None,
                 f"known_at {k} is in the one form YYYY-MM-DDTHH:MM:SS.ffffffZ")
            need(k >= known_at, f"known_at {k} is not before the {known_at} before it")
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

    v3 = json.loads((HERE / "v3.json").read_text("utf-8"))
    prev = v2["hash"]
    for line, want in zip(v3["lines"], v3["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v3.json: position {event['position']} is canonical")
        check(event["v"] == 3, f"v3.json: position {event['position']} is format 3")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v3.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v3.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    ingested, declared = (json.loads(l) for l in v3["lines"])
    check(ingested["upstream"] == {"lake": "upstream-lake", "position": 42},
          "v3.json: the ingested event names its upstream lake and position")
    b = ingested["body"]
    preimage = b"\x00".join([b"lakelands:ingest:v1", ingested["source"].encode(), b["type"].encode(),
                               b["key"].encode(), str(b["version"] - 1).encode(), canonical(b["patch"])])
    check(hashlib.sha256(preimage).hexdigest()[:32] == ingested["id"],
          "v3.json: the ingested event's id leaves `upstream` out (§3)")
    check("date_bucket(" in json.dumps(declared["body"]["declaration"]),
          "v3.json: the declared event's derived field calls date_bucket")
    negative = json.loads(v3["negative"])
    check(negative["v"] == 2 and "upstream" in negative,
          "v3.json: the negative line is format 2 carrying `upstream`, which a reader refuses")
    v4 = json.loads((HERE / "v4.json").read_text("utf-8"))
    prev = v3["hashes"][-1]
    for line, want in zip(v4["lines"], v4["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v4.json: position {event['position']} is canonical")
        check(event["v"] == 4 and event["body"]["kind"] == "write_proposed",
              f"v4.json: position {event['position']} is a format 4 proposal")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v4.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v4.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    kinds = [json.loads(l)["body"]["write"]["write"] for l in v4["lines"]]
    check(kinds == ["declare", "migrate"], "v4.json: a proposed declaration, then a proposed migration")
    check(json.loads(v4["lines"][1])["body"]["write"]["report"]["lossless"] is True,
          "v4.json: the proposed migration carries its preflight's report")
    negative4 = json.loads(v4["negative"])
    check(negative4["v"] == 3 and negative4["body"]["write"]["write"] == "declare",
          "v4.json: the negative line proposes a declaration in format 3, which a reader refuses")
    v5 = json.loads((HERE / "v5.json").read_text("utf-8"))
    prev = v4["hashes"][-1]
    for line, want in zip(v5["lines"], v5["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v5.json: position {event['position']} is canonical")
        check(event["v"] == 5 and annotated(event["body"]),
              f"v5.json: position {event['position']} is a format 5 declaration carrying annotations")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v5.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v5.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    negative5 = json.loads(v5["negative"])
    body = {k: v for k, v in negative5.items() if k != "hash"}
    check(sha(canonical(body)) == negative5["hash"], "v5.json: the negative line's hash is its own")
    check(negative5["v"] == 4 and annotated(negative5["body"]),
          "v5.json: the negative line carries annotations in format 4, which a reader refuses")
    v6 = json.loads((HERE / "v6.json").read_text("utf-8"))
    prev = v5["hashes"][-1]
    for line, want in zip(v6["lines"], v6["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v6.json: position {event['position']} is canonical")
        check(event["v"] == 6 and derives(event["body"]),
              f"v6.json: position {event['position']} is a format 6 declaration of what version 6 added")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v6.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v6.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    negative6 = json.loads(v6["negative"])
    body = {k: v for k, v in negative6.items() if k != "hash"}
    check(sha(canonical(body)) == negative6["hash"], "v6.json: the negative line's hash is its own")
    check(negative6["v"] == 5 and derives(negative6["body"]),
          "v6.json: the negative line declares a derived relation in format 5, which a reader refuses")
    v7 = json.loads((HERE / "v7.json").read_text("utf-8"))
    for line, want in zip(v7["lines"], v7["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v7.json: position {event['position']} is canonical")
        check(event["v"] == 7 and "retired()" in event["body"]["declaration"]["when"],
              f"v7.json: position {event['position']} is a format 7 trigger whose guard calls retired")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v7.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v7.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    negative7 = json.loads(v7["negative"])
    body = {k: v for k, v in negative7.items() if k != "hash"}
    check(sha(canonical(body)) == negative7["hash"], "v7.json: the negative line's hash is its own")
    check(negative7["v"] == 6 and "retired()" in negative7["body"]["declaration"]["when"],
          "v7.json: the negative line calls retired in format 6, which a reader refuses")
    v8 = json.loads((HERE / "v8.json").read_text("utf-8"))
    for line, want in zip(v8["lines"], v8["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v8.json: position {event['position']} is canonical")
        check(event["v"] == 8 and (on_type(event["body"])
                                   or event["body"]["declaration"]["view"]["params"]),
              f"v8.json: position {event['position']} is a format 8 view with parameters or verb on its type")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v8.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v8.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    negative8 = json.loads(v8["negative"])
    body = {k: v for k, v in negative8.items() if k != "hash"}
    check(sha(canonical(body)) == negative8["hash"], "v8.json: the negative line's hash is its own")
    check(negative8["v"] == 7 and negative8["body"]["declaration"]["view"]["params"],
          "v8.json: the negative line declares a view's parameters in format 7, which a reader refuses")
    v9 = json.loads((HERE / "v9.json").read_text("utf-8"))
    for line, want in zip(v9["lines"], v9["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v9.json: position {event['position']} is canonical")
        check(event["v"] == 9 and event["body"]["declaration"]["view"]["merge"],
              f"v9.json: position {event['position']} is a format 9 view that merges sources")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v9.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v9.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    negative9 = json.loads(v9["negative"])
    body = {k: v for k, v in negative9.items() if k != "hash"}
    check(sha(canonical(body)) == negative9["hash"], "v9.json: the negative line's hash is its own")
    check(negative9["v"] == 8 and negative9["body"]["declaration"]["view"]["merge"],
          "v9.json: the negative line declares a merge in format 8, which a reader refuses")
    v10 = json.loads((HERE / "v10.json").read_text("utf-8"))
    narrows = lambda e: e["body"].get("write", {}).get("write") == "migrate_value_type"
    for line, want in zip(v10["lines"], v10["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v10.json: position {event['position']} is canonical")
        check(event["v"] == (10 if narrows(event) else 9),
              f"v10.json: position {event['position']} is format 10 only where it proposes a tightening")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v10.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v10.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    negative10 = json.loads(v10["negative"])
    body = {k: v for k, v in negative10.items() if k != "hash"}
    check(sha(canonical(body)) == negative10["hash"], "v10.json: the negative line's hash is its own")
    check(negative10["v"] == 9 and narrows(negative10),
          "v10.json: the negative line proposes a tightening in format 9, which a reader refuses")
    for name in ("negative_verb", "negative_call"):
        negative = json.loads(v8[name])
        body = {k: v for k, v in negative.items() if k != "hash"}
        check(sha(canonical(body)) == negative["hash"], f"v8.json: the {name} line's hash is its own")
        check(negative["v"] == 7 and on_type(negative["body"]),
              f"v8.json: the {name} line is a verb on its type in format 7, which a reader refuses")
    misplaced5 = json.loads(v5["misplaced"])
    body = {k: v for k, v in misplaced5.items() if k != "hash"}
    check(sha(canonical(body)) == misplaced5["hash"], "v5.json: the misplaced line's hash is its own")
    check("cause" in misplaced5["context"],
          "v5.json: the misplaced line carries `cause` inside `context`, which a reader refuses")
    misplaced = json.loads(v3["misplaced"])
    body = {k: v for k, v in misplaced.items() if k != "hash"}
    check(sha(canonical(body)) == misplaced["hash"], "v3.json: the misplaced line's hash is its own")
    check(misplaced["v"] == 3 and "upstream" in misplaced and misplaced["body"]["kind"] != "record_ingested",
          "v3.json: the misplaced line is a format 3 `record_retired` carrying `upstream`, which a reader refuses")

    for n, want in [(1e21, "1e+21"), (1e-7, "1e-7"), (0.000001, "0.000001"), (333333333.3333333, "333333333.3333333"),
                    (-0.0, "0"), (9007199254740992.0, "9007199254740992"), (5e-324, "5e-324")]:
        check(es_number(n) == want, f"number form {want}")


if __name__ == "__main__":
    main()
