#!/usr/bin/env python3
"""An independent check of the log-format test vectors.

It shares no code with Lakelands: its own RFC 8785 serialiser (members
sorted by UTF-16 code units, the ECMAScript number form), SHA-256 from
the standard library, and the rules of docs/log-format.md read as text.
Run it from this directory: `python3 verify.py`. It prints one line per
check and exits non-zero on the first mismatch.

A release reads and writes one format version, `FORMAT` below: every
line of a lake carries it as `v`, from position 0, and `lake_created`
says the same number as its `format`. A line in any other format, older
or newer, is refused
(docs/decisions/054-a-lake-is-written-in-one-format-a-release.md).

Given a directory, `python3 verify.py <dir>` checks a lake's log as an
export or a SQLite lake directory holds it instead, reading only
`<dir>/log/` (log-format §4.2): canonical lines each ending in a line
feed, positions consecutive from 0 and naming their files in 20 digits,
the hash chain from 64 zeros, every checkpoint against the position,
the hash and the files before it, every line in the one format, `cause`
never inside `context`, `upstream` only on a `record_ingested` event,
and every `known_at` in its one form and never before the `known_at`
before it (log-format §1.3). It prints one line per file and the last
position and hash, or the position where the files stop being the log,
and exits non-zero there.

`python3 verify.py --stamp N` is a release's format step: it rewrites
the vectors in place as the same lake's log in format N, and `FORMAT`
below with them, then checks what it wrote. See `stamp`.
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


# The one format version; `--stamp N` rewrites this line.
FORMAT = 14
ZEROS = "0" * 64
ONE_FORM = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{6}Z")


def derives(body):
    """Whether a declaration the event body carries holds a derived
    relation, or a trigger's `max_depth`."""
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
    """Whether the event body is a verb on its type: declared with
    `scope: type` (in a `declared` event or a proposed `declare`), or
    called (`verb_called`) or proposed (a `call`) without a `key`."""
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
    position, prev, known_at = 0, ZEROS, ""
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
            need(type(v) is int and v == FORMAT, f"format {v} is the one format, {FORMAT}")
            made = event["body"].get("format") if event["body"].get("kind") == "lake_created" else FORMAT
            need(made == FORMAT, f"`lake_created` says format {made}, and its lake is written in format {FORMAT}")
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


def restamp(event, n, prev):
    """The event in format `n` after the hash `prev`: its `v`, a
    `lake_created`'s `format`, its `prev`, and its `hash` made right."""
    event["v"] = n
    if event["body"]["kind"] == "lake_created":
        event["body"]["format"] = n
    event["prev"] = prev
    event["hash"] = sha(canonical({k: x for k, x in event.items() if k != "hash"}))
    return event


def stamp_log(root, n):
    """Rewrite the segment files under `root/log/` in format `n`: each
    line restamped after the one before it, a checkpoint's body made to
    repeat the hash before it and to list the files before it as they
    are now. Answers the events and each file's `first`, `last` and
    `digest`."""
    prev, events, sealed = ZEROS, [], []
    for path in sorted((root / "log").glob("*.jsonl")):
        lines = []
        for line in path.read_bytes()[:-1].split(b"\n"):
            event = json.loads(line.decode("utf-8"))
            if event["body"]["kind"] == "checkpoint":
                event["body"].update(hash=prev, segments=list(sealed))
            lines.append(restamp(event, n, prev))
            prev = event["hash"]
        raw = b"".join(canonical(e) + b"\n" for e in lines)
        path.write_bytes(raw)
        sealed.append({"first": lines[0]["position"], "last": lines[-1]["position"], "digest": sha(raw)})
        events += lines
    return events, sealed


def stamp(n):
    """Rewrite the vectors in place as the same lake's log in format `n`.

    The chain is walked once, in the order its positions run: the segment
    files, then `rekey.json`, then each `vN.json`. Every line gets `v`
    `n` (and `lake_created` its `format`), the `prev` its new
    predecessor's hash gives it, and its own hash; the checkpoint gets
    the sealed file's new last hash and digest; a file's stored `hash`
    or `hashes` and `expected.json` get what the lines now hash to. A
    `misplaced` line is restamped after the position before its own, so
    it stays refused for its one reason alone, and so is
    `negative/known-at-goes-back`. `negative/another-format` is written
    from the first segment, its last event in format `n - 1`. No id
    moves: an id is random or made from members that leave `v` out, and
    `main` recomputes them after this. Last, `FORMAT` in this file's
    source becomes `n`."""
    global FORMAT
    events, sealed = stamp_log(HERE, n)
    files = [HERE / "rekey.json", *HERE.glob("v[0-9]*.json")]
    docs = {p: json.loads(p.read_text("utf-8")) for p in files}
    held = lambda doc: [doc["line"]] if "line" in doc else doc["lines"]
    for path in sorted(files, key=lambda p: json.loads(held(docs[p])[0])["position"]):
        doc = docs[path]
        # A member this does not know may hold a hash of the chain.
        other = set(doc) - {"about", "line", "hash", "lines", "hashes", "misplaced"}
        if other:
            sys.exit(f"FAIL {path.name}: --stamp does not know the members {sorted(other)}")
        lines = []
        for line in held(doc):
            event = json.loads(line)
            if event["position"] != len(events):
                sys.exit(f"FAIL {path.name}: position {event['position']} does not continue the chain at {len(events)}")
            events.append(restamp(event, n, events[-1]["hash"]))
            lines.append(canonical(event).decode("utf-8"))
        if "line" in doc:
            doc["line"], doc["hash"] = lines[0], events[-1]["hash"]
        else:
            doc["lines"], doc["hashes"] = lines, [e["hash"] for e in events[-len(lines):]]
        if "misplaced" in doc:
            event = json.loads(doc["misplaced"])
            before = events[event["position"] - 1]["hash"]
            doc["misplaced"] = canonical(restamp(event, n, before)).decode("utf-8")
        path.write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n", "utf-8")

    expected = json.loads((HERE / "expected.json").read_text("utf-8"))
    for want in expected["events"]:
        want["hash"] = events[want["position"]]["hash"]
    for seg, now in zip(expected["segments"], sealed, strict=True):
        seg.update(now)
    (HERE / "expected.json").write_text(json.dumps(expected, indent=2, ensure_ascii=False) + "\n", "utf-8")

    stamp_log(HERE / "negative" / "known-at-goes-back", n)
    first = events[:sealed[0]["last"] + 1]
    last = restamp(json.loads(canonical(first[-1])), n - 1, first[-1]["prev"])
    path = HERE / "negative" / "another-format" / expected["segments"][0]["file"]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(canonical(e) + b"\n" for e in first[:-1] + [last]))

    source = Path(__file__).read_text("utf-8")
    Path(__file__).write_text(re.sub(r"(?m)^FORMAT = [0-9]+$", f"FORMAT = {n}", source, count=1), "utf-8")
    FORMAT = n
    print(f"ok   stamped in format {n} through position {len(events) - 1}, hash {events[-1]['hash']}")


def main():
    if sys.argv[1:2] == ["--stamp"]:
        stamp(int(sys.argv[2]))
    elif len(sys.argv) > 1:
        return verify_dir(Path(sys.argv[1]))
    expected = json.loads((HERE / "expected.json").read_text("utf-8"))
    prev = ZEROS
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
        check(event["v"] == FORMAT, f"position {pos}: format {FORMAT}")
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
        else:
            check(event["id"] == want["id"], f"position {pos}: the fixed id {want['id']}")
        b = event["body"]
        if b["kind"] == "lake_created":
            check(b["format"] == FORMAT, f"position {pos}: `lake_created` says format {FORMAT}")
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
    # A shape's key is the key the type's records carry: of a view its `group_by`, with `key` written or not.
    declared = view["declaration"]
    for form in (declared, {k: v for k, v in declared.items() if k != "key"}):
        check(form["view"].get("group_by", []) == view["shape"]["key"],
              "schema-hash.json: the shape's key is the view's `group_by`, its `key` written or left out")
    check(view["key_left_out_schema_hash"] == view["schema_hash"],
          "schema-hash.json: the view hashes alike with its `key` left out")

    v2 = json.loads((HERE / "rekey.json").read_text("utf-8"))
    event = json.loads(v2["line"])
    check(canonical(event) == v2["line"].encode("utf-8"), "rekey.json: the line is canonical")
    body = {k: v for k, v in event.items() if k != "hash"}
    check(event["v"] == FORMAT and event["body"]["kind"] == "migrated" and event["body"]["rekeyed"],
          f"rekey.json: a `migrated` event with `rekeyed`, in format {FORMAT}")
    check(sha(canonical(body)) == event["hash"] == v2["hash"], f"rekey.json: hash {v2['hash'][:12]}…")
    check(event["prev"] == expected["events"][-1]["hash"], "rekey.json: it follows position 7")

    v3 = json.loads((HERE / "v3.json").read_text("utf-8"))
    prev = v2["hash"]
    for line, want in zip(v3["lines"], v3["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v3.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT, f"v3.json: position {event['position']} is format {FORMAT}")
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
    v4 = json.loads((HERE / "v4.json").read_text("utf-8"))
    prev = v3["hashes"][-1]
    for line, want in zip(v4["lines"], v4["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v4.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and event["body"]["kind"] == "write_proposed",
              f"v4.json: position {event['position']} is a proposal, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v4.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v4.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    kinds = [json.loads(l)["body"]["write"]["write"] for l in v4["lines"]]
    check(kinds == ["declare", "migrate"], "v4.json: a proposed declaration, then a proposed migration")
    check(json.loads(v4["lines"][1])["body"]["write"]["report"]["lossless"] is True,
          "v4.json: the proposed migration carries its preflight's report")
    v5 = json.loads((HERE / "v5.json").read_text("utf-8"))
    prev = v4["hashes"][-1]
    for line, want in zip(v5["lines"], v5["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v5.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and annotated(event["body"]),
              f"v5.json: position {event['position']} is a declaration carrying annotations, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v5.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v5.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    v6 = json.loads((HERE / "v6.json").read_text("utf-8"))
    prev = v5["hashes"][-1]
    for line, want in zip(v6["lines"], v6["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v6.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and derives(event["body"]),
              f"v6.json: position {event['position']} declares a derived relation or a `max_depth`, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v6.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v6.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    v7 = json.loads((HERE / "v7.json").read_text("utf-8"))
    for line, want in zip(v7["lines"], v7["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v7.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and "retired()" in event["body"]["declaration"]["when"],
              f"v7.json: position {event['position']} is a trigger whose guard calls retired, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v7.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v7.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    v8 = json.loads((HERE / "v8.json").read_text("utf-8"))
    for line, want in zip(v8["lines"], v8["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v8.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and (on_type(event["body"])
                                        or event["body"]["declaration"]["view"]["params"]),
              f"v8.json: position {event['position']} is a view with parameters or a verb on its type, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v8.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v8.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    v9 = json.loads((HERE / "v9.json").read_text("utf-8"))
    for line, want in zip(v9["lines"], v9["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v9.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and event["body"]["declaration"]["view"]["merge"],
              f"v9.json: position {event['position']} is a view that merges sources, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v9.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v9.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    v10 = json.loads((HERE / "v10.json").read_text("utf-8"))
    narrows = lambda e: e["body"].get("write", {}).get("write") == "migrate_value_type"
    for line, want in zip(v10["lines"], v10["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v10.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT, f"v10.json: position {event['position']} is format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v10.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v10.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    check([narrows(json.loads(l)) for l in v10["lines"]] == [False, True],
          "v10.json: a declared value type, then a proposed tightening of it")
    # One line for each thing v11.json shows.
    v11 = json.loads((HERE / "v11.json").read_text("utf-8"))
    derived = lambda e: [f.get("derived", "") for f in e["body"].get("declaration", {}).get("fields", [])]
    by_a_width = re.compile(r"date_bucket\([^,]+, [0-9.]+[dhms],")
    eleventh = [lambda e: any("floor(" in d for d in derived(e)),
                lambda e: any(by_a_width.search(d) for d in derived(e)),
                lambda e: "in_set(" in e["body"].get("declaration", {}).get("where", ""),
                lambda e: bool(e["body"].get("migration", {}).get("relation_renames")),
                lambda e: e["body"].get("type") == "sys.setting" and e["body"].get("key", "").startswith("view_kept."),
                lambda e: e["body"].get("kind") in ("record_upserted", "record_ingested") and e["body"].get("made") is True,
                lambda e: bool(e["body"].get("write", {}).get("report", {}).get("restated")),
                lambda e: any("initial" in f for f in e["body"].get("declaration", {}).get("fields", []))]
    for line, want in zip(v11["lines"], v11["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v11.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and any(shown(event) for shown in eleventh),
              f"v11.json: position {event['position']} carries what it shows, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v11.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v11.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    v12 = json.loads((HERE / "v12.json").read_text("utf-8"))
    goes_on = lambda e: e["body"].get("kind") == "declared" and e["body"].get("with_the_set") is True
    renames = lambda e: e["body"].get("kind") == "value_type_migrated" or (
        e["body"].get("kind") == "write_proposed"
        and e["body"]["write"].get("write") == "migrate_value_type"
        and bool(e["body"]["write"].get("renames")))
    twelfth = lambda e: goes_on(e) or renames(e)
    for line, want in zip(v12["lines"], v12["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v12.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and twelfth(event),
              f"v12.json: position {event['position']} carries what it shows, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v12.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v12.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    v13 = json.loads((HERE / "v13.json").read_text("utf-8"))
    pinned = lambda e: e["body"].get("kind") == "write_proposed" and e["body"].get("pinned") is True
    declaration = lambda e: e["body"].get("declaration", {}) if e["body"].get("kind") == "declared" else {}
    names_a_date = lambda e: declaration(e).get("kind") == "date" or any(
        f["type"]["kind"] == "date" for f in declaration(e).get("fields", []))
    thirteenth = lambda e: pinned(e) or names_a_date(e)
    for line, want in zip(v13["lines"], v13["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v13.json: position {event['position']} is canonical")
        # A pinned proposal, a declaration that names the kind `date`, or
        # a record holding one.
        check(event["v"] == FORMAT and (thirteenth(event) or event["body"]["kind"] == "record_upserted"),
              f"v13.json: position {event['position']} carries what it shows, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v13.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v13.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    v14 = json.loads((HERE / "v14.json").read_text("utf-8"))
    named = lambda e: e["body"].get("kind") == "output_recorded" and "output_at" in e["body"]
    for line, want in zip(v14["lines"], v14["hashes"]):
        event = json.loads(line)
        check(canonical(event) == line.encode("utf-8"), f"v14.json: position {event['position']} is canonical")
        check(event["v"] == FORMAT and ("output" in event["body"]) != named(event),
              f"v14.json: position {event['position']} carries its output or names it, in format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == want, f"v14.json: hash {want[:12]}…")
        check(event["prev"] == prev, f"v14.json: position {event['position']} follows the one before it")
        prev = event["hash"]
    check([named(json.loads(l)) for l in v14["lines"]] == [False, True],
          "v14.json: a fill that carries its output, then one that names it")
    # A misplaced line is in the one format and its hash is right, so a
    # reader refuses it for where a member sits and for nothing else.
    misplaced14 = json.loads(v14["misplaced"])
    body = {k: v for k, v in misplaced14.items() if k != "hash"}
    check(sha(canonical(body)) == misplaced14["hash"], "v14.json: the misplaced line's hash is its own")
    check(misplaced14["v"] == FORMAT and named(misplaced14) and "output" in misplaced14["body"],
          "v14.json: the misplaced line carries both `output` and `output_at`, which a reader refuses")
    misplaced5 = json.loads(v5["misplaced"])
    body = {k: v for k, v in misplaced5.items() if k != "hash"}
    check(sha(canonical(body)) == misplaced5["hash"], "v5.json: the misplaced line's hash is its own")
    check(misplaced5["v"] == FORMAT and "cause" in misplaced5["context"],
          "v5.json: the misplaced line carries `cause` inside `context`, which a reader refuses")
    misplaced = json.loads(v3["misplaced"])
    body = {k: v for k, v in misplaced.items() if k != "hash"}
    check(sha(canonical(body)) == misplaced["hash"], "v3.json: the misplaced line's hash is its own")
    check(misplaced["v"] == FORMAT and "upstream" in misplaced and misplaced["body"]["kind"] != "record_ingested",
          "v3.json: the misplaced line is a `record_retired` carrying `upstream`, which a reader refuses")

    for n, want in [(1e21, "1e+21"), (1e-7, "1e-7"), (0.000001, "0.000001"), (333333333.3333333, "333333333.3333333"),
                    (-0.0, "0"), (9007199254740992.0, "9007199254740992"), (5e-324, "5e-324")]:
        check(es_number(n) == want, f"number form {want}")


if __name__ == "__main__":
    main()
