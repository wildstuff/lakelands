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
`sealed` only on a `record_ingested` or an `output_recorded`, `apart`
and a selection by `seen_before` or `deliveries` only from format 18 on,
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
            raise ValueError(f"the integer {v} in it is beyond ±(2^53 − 1), which not every reader reads exactly")
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


def canon(event, refuse):
    """The event's canonical bytes. A value that has none (an integer
    beyond ±(2^53 − 1), log-format §1.2; a number that is not finite) is
    a verdict where the event stands: `refuse` says it and exits."""
    try:
        return canonical(event)
    except ValueError as e:
        refuse(f"the line has no canonical form: {e}")


# The one format version; `--stamp N` rewrites this line.
FORMAT = 19
# The oldest format a lake directory may hold events in: a lake the
# release before wrote and this one upgraded (log-format §6).
OLDEST = 14
ZEROS = "0" * 64
ONE_FORM = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{6}Z")


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


def shows(what):
    """Name a check by what its line shows; `check` prints the name."""
    def named(f):
        f.__doc__ = what
        return f
    return named


def declaration(e):
    """The declaration a `declared` event carries, or none."""
    return e["body"].get("declaration", {}) if e["body"].get("kind") == "declared" else {}


def proposed(e, write):
    """The write a `write_proposed` event carries, when it is a `write`."""
    w = e["body"].get("write", {}) if e["body"].get("kind") == "write_proposed" else {}
    return w if w.get("write") == write else None


def derived(e):
    """The derived expressions of a declared type's fields."""
    return [f.get("derived", "") for f in declaration(e).get("fields", [])]


def ingest_id(e):
    """An ingest event's id from its members, `upstream` not among them (log-format §3)."""
    b = e["body"]
    return content_id(b"lakelands:ingest:v1", e["source"].encode(), b["type"].encode(), b["key"].encode(),
                      str(b["version"] - 1).encode(), canonical(b["patch"]))


# The base64 a sealed value is written in (058), and the kinds that carry one.
BASE64 = re.compile(r"(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?")
SEALS = ("record_ingested", "output_recorded")
BY_A_WIDTH = re.compile(r"date_bucket\([^,]+, [0-9.]+[dhms],")
# What each line of `further.json` is there to show, by its `shows`.
SHOWS = {
    "rekeyed": shows("a `migrated` event with `rekeyed`")(
        lambda e: e["body"]["kind"] == "migrated" and bool(e["body"].get("rekeyed"))),
    "upstream": shows("an ingest that names its upstream lake and position, which its id leaves out")(
        lambda e: e["body"]["kind"] == "record_ingested" and e.get("upstream") == {"lake": "upstream-lake", "position": 42}
        and ingest_id(e) == e["id"]),
    "date_bucket": shows("a declared type whose derived field calls `date_bucket`")(
        lambda e: any("date_bucket(" in d for d in derived(e))),
    "proposed_declare": shows("a proposed set of declarations")(
        lambda e: bool((proposed(e, "declare") or {}).get("declarations"))),
    "proposed_migrate": shows("a proposed migration with its preflight's report")(
        lambda e: (proposed(e, "migrate") or {}).get("report", {}).get("lossless") is True),
    "proposed_migrate_recomputed": shows("a proposed migration whose report counts the live records it computes again")(
        lambda e: (proposed(e, "migrate") or {}).get("report", {}).get("recomputed") == 1),
    "proposed_plan": shows("a proposed set of declarations carrying its plan")(
        lambda e: any(t.get("recomputed") and t.get("unlive")
                      for t in ((proposed(e, "declare") or {}).get("plan") or {}).get("types", []))),
    "annotations": shows("a declaration carrying annotations")(
        lambda e: e["body"]["kind"] == "declared" and annotated(e["body"])),
    "derived_relation": shows("a declared type with a derived relation")(
        lambda e: any("derived" in r for r in declaration(e).get("relations", []))),
    "max_depth": shows("a declared trigger with `max_depth`")(
        lambda e: declaration(e).get("declare") == "trigger" and "max_depth" in declaration(e)),
    "retired_guard": shows("a declared trigger whose guard calls `retired`")(
        lambda e: declaration(e).get("declare") == "trigger" and "retired()" in declaration(e).get("when", "")),
    "view_params": shows("a declared view with parameters, one of which its `where` reads")(
        lambda e: bool(declaration(e).get("view", {}).get("params")) and "param(" in declaration(e)["view"].get("where", "")),
    "verb_on_type": shows("a declared verb on its type")(
        lambda e: declaration(e).get("declare") == "verb" and declaration(e).get("scope") == "type"),
    "verb_called_on_type": shows("a verb called on its type, with no `key`")(
        lambda e: e["body"]["kind"] == "verb_called" and "key" not in e["body"]),
    "view_merge": shows("a declared view that merges a source")(
        lambda e: bool(declaration(e).get("view", {}).get("merge"))),
    "value_type": shows("a declared value type")(
        lambda e: declaration(e).get("declare") == "value_type"),
    "proposed_tightening": shows("a proposed migration of a value type with its preflight's report")(
        lambda e: "report" in (proposed(e, "migrate_value_type") or {})
        and not proposed(e, "migrate_value_type").get("renames")),
    "floor": shows("a declared type whose derived field calls `floor`")(
        lambda e: any("floor(" in d for d in derived(e))),
    "date_bucket_by_width": shows("a declared type whose derived field calls `date_bucket` by a width")(
        lambda e: any(BY_A_WIDTH.search(d) for d in derived(e))),
    "in_set": shows("a declared object set whose `where` calls `in_set`")(
        lambda e: declaration(e).get("declare") == "object_set" and "in_set(" in declaration(e).get("where", "")),
    "relation_renames": shows("a `migrated` event whose migration renames a relation")(
        lambda e: e["body"]["kind"] == "migrated" and bool(e["body"]["migration"].get("relation_renames"))),
    "view_kept": shows("a `view_kept.` setting at its first version, which carries `made`")(
        lambda e: e["body"]["kind"] == "record_upserted" and e["body"]["type"] == "sys.setting"
        and e["body"]["key"].startswith("view_kept.") and e["body"]["version"] == 1 and e["body"].get("made") is True),
    "made_again": shows("a record made again: `made` at a version above 1")(
        lambda e: e["body"]["kind"] == "record_upserted" and e["body"]["version"] > 1 and e["body"].get("made") is True),
    "restated": shows("a proposed migration whose report holds `restated`")(
        lambda e: bool((proposed(e, "migrate") or {}).get("report", {}).get("restated"))),
    "initial": shows("a declared type whose field declares `initial`")(
        lambda e: any("initial" in f for f in declaration(e).get("fields", []))),
    "with_the_set": shows("a declaration that carries `with_the_set`")(
        lambda e: e["body"]["kind"] == "declared" and e["body"].get("with_the_set") is True),
    "value_type_migrated": shows("a `value_type_migrated` that renames a value")(
        lambda e: e["body"]["kind"] == "value_type_migrated" and bool(e["body"].get("renames"))),
    "proposed_renames": shows("a proposed migration of a value type that carries `renames`")(
        lambda e: bool((proposed(e, "migrate_value_type") or {}).get("renames"))),
    "pinned": shows("a proposed upsert that carries `pinned`")(
        lambda e: proposed(e, "upsert") is not None and e["body"].get("pinned") is True),
    "date_field": shows("a declared type with a field of the kind `date`")(
        lambda e: any(f["type"]["kind"] == "date" for f in declaration(e).get("fields", []))),
    "date_value_type": shows("a declared value type over the kind `date`")(
        lambda e: declaration(e).get("declare") == "value_type" and declaration(e).get("kind") == "date"),
    "date_value": shows("a record holding a date, a string of ten characters")(
        lambda e: e["body"]["kind"] == "record_upserted"
        and any(isinstance(x, str) and re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", x) for x in e["body"]["patch"].values())),
    "output_carried": shows("a recorded output that carries its `output`")(
        lambda e: e["body"]["kind"] == "output_recorded" and "output" in e["body"] and "output_at" not in e["body"]),
    "output_named": shows("a recorded output that names its output by `output_at`")(
        lambda e: e["body"]["kind"] == "output_recorded" and "output_at" in e["body"] and "output" not in e["body"]),
    "erasable": shows("a declared type with a field declared `erasable`")(
        lambda e: any(f.get("erasable") is True for f in declaration(e).get("fields", []))),
    "sealed_ingest": shows("an ingest whose `sealed` names its patch's sealed members, its id made from the patch as stored")(
        lambda e: e["body"]["kind"] == "record_ingested" and bool(e.get("sealed", {}).get("fields"))
        and all(isinstance(e["body"]["patch"].get(f), str) and BASE64.fullmatch(e["body"]["patch"][f]) for f in e["sealed"]["fields"])
        and ingest_id(e) == e["id"]),
    "sealed_output": shows("a recorded output sealed under the key of the source `sealed` names")(
        lambda e: e["body"]["kind"] == "output_recorded" and "source" in e.get("sealed", {})
        and isinstance(e["body"].get("output"), str) and BASE64.fullmatch(e["body"]["output"]) is not None),
    "set_across": shows("a declared object set whose `where` reads across an edge")(
        lambda e: declaration(e).get("declare") == "object_set" and "shelf.capacity" in declaration(e).get("where", "")),
    "trigger_across": shows("a declared trigger whose `when` reads across an edge and asks `exists`")(
        lambda e: declaration(e).get("declare") == "trigger" and "exists(shelf)" in declaration(e).get("when", "")
        and "shelf.capacity" in declaration(e)["when"]),
    "constraints": shows("a declared type with a constraint that admits at ingest and one that reads `previous`")(
        lambda e: [c.get("at_ingest", "refuse") for c in declaration(e).get("constraints", [])] == ["admit", "refuse"]
        and "previous(" in declaration(e)["constraints"][1]["holds"]),
    "source_forgotten": shows("a `source_forgotten` naming the source and the versions of its key")(
        lambda e: e["body"]["kind"] == "source_forgotten" and e["body"].get("source") == "crm"
        and e["body"].get("keys") == ["1"] and "sealed" not in e),
    "verb_authority": shows("a declared type with a field only its verbs write, `authority: verb`")(
        lambda e: any(f.get("authority") == "verb" for f in declaration(e).get("fields", []))),
    "admits": shows("a declared type with an authored field that admits a named source")(
        lambda e: any(f.get("authority") == "authored" and f.get("admits") == ["samples"]
                      for f in declaration(e).get("fields", []))),
    "verb_granted": shows("a declared verb kept to the callers granted it, `granted`")(
        lambda e: declaration(e).get("declare") == "verb" and declaration(e).get("granted") is True),
    "setting_declared": shows("a declared host setting with its type, unit, meaning and group")(
        lambda e: declaration(e).get("declare") == "setting" and declaration(e).get("key") == "vat_rate"
        and declaration(e).get("type") == {"kind": "decimal"} and declaration(e).get("group") == "operator"),
    "setting_group": shows("a `sys.setting` record carrying the `group` stamped from its declaration")(
        lambda e: e["body"]["kind"] == "record_upserted" and e["body"]["type"] == "sys.setting"
        and e["body"]["patch"].get("group") == "operator"),
    "delivery": shows("an ingest in the delivery its caller named, its id made without it")(
        lambda e: e["body"]["kind"] == "record_ingested" and e.get("delivery") == "batch-7" and ingest_id(e) == e["id"]),
    "withdrawn": shows("a declared withdrawal of a trigger, at the version after the one it withdraws")(
        lambda e: declaration(e) == {"declare": "withdraw", "kind": "trigger", "name": "shelved"}
        and e["body"]["version"] == 2),
    "retired_by_verbs": shows("a declared type whose records only its verbs retire, `retired_by: verbs`")(
        lambda e: declaration(e).get("retired_by") == "verbs"),
    "retire_effect": shows("a declared verb whose effect is `retire`")(
        lambda e: declaration(e).get("declare") == "verb" and declaration(e).get("effect") == "retire"),
    "untyped_input": shows("a declared function whose input states no `type`, which a type binds as a field of any kind")(
        lambda e: declaration(e).get("declare") == "function" and declaration(e).get("inputs")
        and all("type" not in i for i in declaration(e)["inputs"])),
    "values_removed": shows("a `values_removed` taking back one delivery of a source")(
        lambda e: e["body"]["kind"] == "values_removed" and e["body"].get("source") == "erp"
        and e["body"].get("selection") == {"delivery": "batch-7"} and "before" not in e["body"]),
    "removal_retires": shows("the `record_retired` of a record only the delivery taken back wrote")(
        lambda e: e["body"]["kind"] == "record_retired" and e["body"].get("key") == "INV-2"),
    "delivery_own": shows("an ingest in the store's own delivery, `d` and the position of its call's first event")(
        lambda e: e["body"]["kind"] == "record_ingested" and e.get("delivery") == f"d{e['position']}"
        and ingest_id(e) == e["id"]),
    "removed_before": shows("a `values_removed` taking back what a source wrote before a time, naming the first position it does not reach")(
        lambda e: e["body"]["kind"] == "values_removed" and e["body"].get("source") == "erp"
        and set(e["body"].get("selection", {})) == {"written_before"} and e["body"].get("before") == e["position"]
        and "keys" not in e["body"]),
    "sealed_apart": shows("an ingest of a delivery sealed apart, whose `sealed` names a version and the delivery's own key in `apart`")(
        lambda e: e["body"]["kind"] == "record_ingested" and e.get("sealed", {}).get("apart") == ["a1"]
        and e.get("delivery") == "late" and ingest_id(e) == e["id"]
        and all(isinstance(e["body"]["patch"].get(f), str) and BASE64.fullmatch(e["body"]["patch"][f]) for f in e["sealed"]["fields"])),
    "sealed_apart_output": shows("a recorded output sealed under a version composed with a delivery's key apart")(
        lambda e: e["body"]["kind"] == "output_recorded" and e.get("sealed", {}).get("apart") == ["a1"]
        and "source" in e["sealed"] and BASE64.fullmatch(e["body"].get("output", "")) is not None),
    "removed_apart": shows("a `values_removed` that destroys a delivery's key apart and no version")(
        lambda e: e["body"]["kind"] == "values_removed" and e["body"].get("apart") == ["a1"]
        and "keys" not in e["body"]),
    "removed_seen_before": shows("a `values_removed` taking back what a source says it observed before a time, naming no position")(
        lambda e: e["body"]["kind"] == "values_removed" and e["body"].get("source") == "erp"
        and set(e["body"].get("selection", {})) == {"seen_before"} and "before" not in e["body"]
        and "keys" not in e["body"] and "apart" not in e["body"]),
    "removed_deliveries": shows("a `values_removed` taking back a set of a source's deliveries, its ids in order, naming no position")(
        lambda e: e["body"]["kind"] == "values_removed" and e["body"].get("source") == "erp"
        and e["body"].get("selection") == {"deliveries": ["batch-7", "d68"]} and "before" not in e["body"]
        and "keys" not in e["body"] and "apart" not in e["body"]),
    "removal_apart_retires": shows("the `record_retired` of a record only the delivery sealed apart wrote")(
        lambda e: e["body"]["kind"] == "record_retired" and e["body"].get("key") == "c5"),
    "removal_by_time_retires": shows("the `record_retired` of a record only what was taken back by time wrote")(
        lambda e: e["body"]["kind"] == "record_retired" and e["body"].get("key") == "INV-3"),
    "lake_upgraded": shows("a `lake_upgraded` from the format before to the one this lake is written in")(
        lambda e: e["body"]["kind"] == "lake_upgraded" and e["body"]["from"] == e["v"] - 1 and e["body"]["to"] == e["v"]),
    "function_risk": shows("a declared function whose backing reaches outside the lake, declared above `low` risk")(
        lambda e: declaration(e).get("declare") == "function" and declaration(e).get("backing") != "expr"
        and declaration(e).get("risk") in ("medium", "high")),
    "computed_by_function": shows("a declared type with a field computed by a function")(
        lambda e: declaration(e).get("declare") == "type"
        and any(f.get("function") for f in declaration(e).get("fields", []))),
    "computed_write": shows("a write of a type that a function computes a field of, which makes its call due")(
        lambda e: e["body"]["kind"] == "record_upserted" and e["body"]["type"] == "alert"),
    "proposed_compute": shows("a proposed call of a function: the record, the field and the call's digest on that record as the write at its cause made it, with no inputs, and that write as its cause")(
        lambda e: (proposed(e, "compute") or {}).get("input") == sha(canonical({
            "call": sha(canonical({"function": "notify", "inputs": {"subject": "disk full"},
                                   "config": {"url": "https://notify.example/send"}})),
            "record": ["alert", "a1"], "made": 55}))
        and "inputs" not in proposed(e, "compute") and e.get("cause") == 55),
    "trigger_change_guard": shows("a declared trigger whose guard reads `created()`, strict, naming the kinds of change that fire it")(
        lambda e: declaration(e).get("declare") == "trigger" and declaration(e).get("when") == "created()"
        and declaration(e).get("when_strict") is True and declaration(e).get("on_changes") == ["upserted", "ingested"]),
    "slip_write": shows("an authored write that makes a slip")(
        lambda e: e["body"]["kind"] == "record_upserted" and e["body"]["type"] == "slip" and e["body"]["made"] is True),
    "fired_change": shows("a `trigger_fired` of a trigger whose guard reads `created()`, naming the kind of change at its cause, and that the change made the record")(
        lambda e: e["body"]["kind"] == "trigger_fired" and e["body"].get("change") == "upserted"
        and e["body"].get("created") is True and e["body"]["trigger"] == "slip_made" and e.get("cause") == 61),
    "reaction_write": shows("a reaction one deeper than the write it reacts to, naming the firing as its cause")(
        lambda e: e["body"]["kind"] == "record_upserted" and e["context"] == {"depth": 1, "origin": "reactive"}
        and e.get("cause") == 62),
    "trigger_dropped": shows("a `trigger_dropped` at the trigger's own bound, in place of a firing")(
        lambda e: e["body"]["kind"] == "trigger_dropped" and e["body"]["trigger"] == "slip_made"
        and e["body"]["depth"] == 1 and e["body"]["max_depth"] == 1 and e["body"]["bound"] == "trigger"
        and e.get("cause") == 63),
}
# The one misplaced member each line of `misplaced` carries.
MISPLACED = {
    "upstream_on_another_kind": shows("`upstream` on an event that is not a `record_ingested`, which a reader refuses")(
        lambda e: "upstream" in e and e["body"]["kind"] != "record_ingested"),
    "cause_in_context": shows("`cause` inside `context`, which a reader refuses")(
        lambda e: "cause" in e["context"]),
    "sealed_on_another_kind": shows("`sealed` on an event that is neither a `record_ingested` nor an `output_recorded`, which a reader refuses")(
        lambda e: "sealed" in e and e["body"]["kind"] not in SEALS),
    "delivery_on_another_kind": shows("`delivery` on an event that is not a `record_ingested`, which a reader refuses")(
        lambda e: "delivery" in e and e["body"]["kind"] != "record_ingested"),
    "output_twice": shows("both `output` and `output_at`, which a reader refuses")(
        lambda e: e["body"]["kind"] == "output_recorded" and "output" in e["body"] and "output_at" in e["body"]),
    "sealed_for_another_source": shows("`sealed` on a `record_ingested` naming another source than its envelope, which a reader refuses")(
        lambda e: e["body"]["kind"] == "record_ingested" and e["sealed"].get("source", e.get("source")) != e.get("source")),
}


def verify_dir(root):
    """Check the segment files under `root/log/` alone; see the module doc."""
    log = root / "log"
    files = sorted(p for p in log.iterdir() if p.suffix == ".jsonl") if log.is_dir() else []
    check(bool(files), f"{root}/log holds segment files")
    position, prev, known_at = 0, ZEROS, ""
    sealed = []
    # The format of the event before, the step a new writer's lease began
    # and its marker must close, and the fence before (log-format §4, §6).
    before, owed, fence = None, None, 0

    def need(ok, why):
        if not ok:
            print(f"FAIL position {position}: {why}")
            sys.exit(1)

    for i, path in enumerate(files):
        first = position
        need(len(path.stem) == 20 and path.stem.isdigit() and int(path.stem) == first,
             f"{path.name} is the file starting at {first}, named in 20 digits")
        raw = path.read_bytes()
        need(raw, f"{path.name} holds an event")
        need(raw.endswith(b"\n"), f"{path.name} ends in a line feed")
        for line in raw[:-1].split(b"\n"):
            try:
                event = json.loads(line.decode("utf-8"))
            except ValueError as e:
                need(False, f"the line is JSON ({e})")
            need(canon(event, lambda why: need(False, why)) == line, "the line is canonical")
            need(event.get("position") == position, f"the line is position {event.get('position')}")
            v = event.get("v")
            need(type(v) is int and OLDEST <= v <= FORMAT, f"format {v} is one from {OLDEST} to {FORMAT}")
            kind = event["body"].get("kind")
            made = event["body"].get("format") if kind == "lake_created" else v
            need(made == v, f"`lake_created` says format {made}, and its line is in format {v}")
            # A format changes only where a new writer upgrades the lake: it
            # rises at that writer's `lease_taken`, whose next event is the
            # `lake_upgraded` from the one to the other; it never falls.
            marks = kind == "lake_upgraded"
            if owed is not None:
                need(marks and v == owed[1] and event["body"].get("from") == owed[0]
                     and event["body"].get("to") == owed[1],
                     f"the `lease_taken` before it raised the format from {owed[0]} to {owed[1]}, and this is not its `lake_upgraded`")
                owed = None
            elif before is not None:
                need(v >= before, f"format {v} falls below the {before} before it")
                need(not marks, "a `lake_upgraded` marks a step a new writer's `lease_taken` begins")
                if v > before:
                    need(kind == "lease_taken", f"format {v} rises from {before} at a new writer's `lease_taken`")
                    owed = (before, v)
            before = v
            f = event.get("fence")
            need(type(f) is int and f >= fence, f"fence {f} is not below the fence {fence} before it")
            fence = f
            need("cause" not in event.get("context", {}),
                 "`context` carries `cause`, which the envelope carries")
            need("upstream" not in event or event["body"].get("kind") == "record_ingested",
                 f"a `{event['body'].get('kind')}` event carries `upstream`, which only a `record_ingested` event carries")
            need("sealed" not in event or event["body"].get("kind") in SEALS,
                 f"a `{event['body'].get('kind')}` event carries `sealed`, which only a `record_ingested` or an `output_recorded` carries")
            need("sealed" not in event or v >= 16, f"an event of format {v} carries `sealed`, which format 16 added")
            named = event.get("sealed", {}).get("source")
            need(event["body"].get("kind") != "record_ingested" or named in (None, event.get("source")),
                 f"a `record_ingested` of `{event.get('source')}` carries `sealed` naming `{named}`, which is not its own source")
            need("delivery" not in event or (event["body"].get("kind") == "record_ingested" and v >= 17),
                 f"a `{event['body'].get('kind')}` event of format {v} carries `delivery`, which only a `record_ingested` of format 17 on carries")
            need(event["body"].get("kind") != "source_forgotten" or v >= 16,
                 f"an event of format {v} is a `source_forgotten`, which format 16 added")
            need(event["body"].get("kind") != "values_removed" or v >= 17,
                 f"an event of format {v} is a `values_removed`, which format 17 added")
            apart = bool(event.get("sealed", {}).get("apart")) or (
                event["body"].get("kind") == "values_removed" and bool(event["body"].get("apart")))
            need(not apart or v >= 18, f"an event of format {v} carries `apart`, which format 18 added")
            for by in ("seen_before", "deliveries"):
                later = event["body"].get("kind") == "values_removed" and by in event["body"].get("selection", {})
                need(not later or v >= 18, f"a `values_removed` of format {v} selects by `{by}`, which format 18 added")
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
        sealed.append({"first": first, "last": position - 1, "digest": sha(raw)})
        print(f"ok   {path.name}: positions {first}..{position - 1}")
    need(owed is None, f"the log ends after a `lease_taken` that raised the format to {owed and owed[1]} with no `lake_upgraded`")
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
    files, then the events of `further.json`. Every line gets `v` `n`
    (and `lake_created` its `format`), the `prev` its new predecessor's
    hash gives it, and its own hash; the checkpoint gets the sealed
    file's new last hash and digest; the hashes `further.json` and
    `expected.json` repeat get what the lines now hash to. A `misplaced`
    line is restamped after the position before its own, so it stays
    refused for its one reason alone, and so is
    `negative/known-at-goes-back`. `negative/another-format` is written
    from the first segment, its last event in format `n - 1`. No id
    moves: an id is random or made from members that leave `v` out, and
    `main` recomputes them after this. Last, `FORMAT` in this file's
    source becomes `n`."""
    global FORMAT
    events, sealed = stamp_log(HERE, n)
    path = HERE / "further.json"
    further = json.loads(path.read_text("utf-8"))
    # A member this does not know may hold a hash of the chain.
    other = set(further) - {"about", "events", "misplaced"}
    for entry, known in [(e, {"shows", "about", "line", "hash"}) for e in further["events"]] + [
            (e, {"shows", "about", "line"}) for e in further["misplaced"]]:
        other |= set(entry) - known
    if other:
        sys.exit(f"FAIL further.json: --stamp does not know the members {sorted(other)}")
    for entry in further["events"]:
        event = json.loads(entry["line"])
        if event["position"] != len(events):
            sys.exit(f"FAIL further.json: position {event['position']} does not continue the chain at {len(events)}")
        events.append(restamp(event, n, events[-1]["hash"]))
        entry["line"], entry["hash"] = canonical(event).decode("utf-8"), event["hash"]
    for entry in further["misplaced"]:
        event = json.loads(entry["line"])
        before = events[event["position"] - 1]["hash"]
        entry["line"] = canonical(restamp(event, n, before)).decode("utf-8")
    path.write_text(json.dumps(further, indent=2, ensure_ascii=False) + "\n", "utf-8")

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
        if len(sys.argv) != 3 or not sys.argv[2].isdecimal():
            sys.exit("usage: python3 verify.py --stamp N, where N is the format version to write the vectors in")
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
            at = f"position {event['position']}"
            check(canon(event, lambda why: check(False, f"{at}: {why}")) == line, f"{at}: the stored line is canonical")
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

    # The events after the segments: one loop, and one check for each
    # thing a line is there to show.
    further = json.loads((HERE / "further.json").read_text("utf-8"))
    for entry in further["events"]:
        event = json.loads(entry["line"])
        pos, shows = event["position"], entry["shows"]
        check(pos == len(events), f"position {pos}: it follows position {len(events) - 1}")
        check(canon(event, lambda why: check(False, f"position {pos}: {why}")) == entry["line"].encode("utf-8"),
              f"position {pos}: the line is canonical")
        check(event["v"] == FORMAT, f"position {pos}: format {FORMAT}")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"] == entry["hash"], f"position {pos}: hash {entry['hash'][:12]}…")
        check(event["prev"] == events[-1]["hash"], f"position {pos}: prev names the previous hash")
        check(shows in SHOWS, f"position {pos}: `{shows}` is a thing this verifier checks")
        check(SHOWS[shows](event), f"position {pos}: {SHOWS[shows].__doc__}")
        events.append(event)
    shown = [entry["shows"] for entry in further["events"]]
    check(sorted(shown) == sorted(SHOWS), "further.json: every thing this verifier checks is shown once")
    # A misplaced line is in the one format and its hash is right, so a
    # reader refuses it for where a member sits and for nothing else.
    for entry in further["misplaced"]:
        event = json.loads(entry["line"])
        pos, shows = event["position"], entry["shows"]
        check(canon(event, lambda why: check(False, f"misplaced at {pos}: {why}")) == entry["line"].encode("utf-8"),
              f"misplaced at {pos}: the line is canonical")
        body = {k: v for k, v in event.items() if k != "hash"}
        check(sha(canonical(body)) == event["hash"], f"misplaced at {pos}: the hash is its own")
        check(event["v"] == FORMAT and event["prev"] == events[pos - 1]["hash"],
              f"misplaced at {pos}: in format {FORMAT}, after position {pos - 1}")
        check(shows in MISPLACED, f"misplaced at {pos}: `{shows}` is a thing this verifier checks")
        check(MISPLACED[shows](event), f"misplaced at {pos}: {MISPLACED[shows].__doc__}")
    check(sorted(entry["shows"] for entry in further["misplaced"]) == sorted(MISPLACED),
          "further.json: every misplaced member this verifier checks is shown once")

    for n, want in [(1e21, "1e+21"), (1e-7, "1e-7"), (0.000001, "0.000001"), (333333333.3333333, "333333333.3333333"),
                    (-0.0, "0"), (9007199254740992.0, "9007199254740992"), (5e-324, "5e-324")]:
        check(es_number(n) == want, f"number form {want}")


if __name__ == "__main__":
    main()
