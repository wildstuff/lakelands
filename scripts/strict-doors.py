#!/usr/bin/env python3
"""The gate record 110 owes: every place a text becomes a value, or a value
becomes a contract type, is listed and reads through `lakelands_api::strict`;
and no type in `lakelands-api` whose `Deserialize` is derived buffers its
input, so a sequence can never be read as a struct on the way in.

A site is one call to a serde entry point in a door crate or in the contract.
The list is `scripts/strict-doors.sites`; an entry names its file, the
function it sits in, the entry point, and the verdict:

    value              it answers JSON, which `strict` reads afterwards (or
                       which is not a contract type at all)
    check              it answers no value: it only proves the text parses
    written            it answers a document type from the text, in the order
                       the author wrote it, after `strict` has read the same
                       document for the refusal a sequence gets
    exempt lakelands#N  it reads a contract type directly, and issue N removes
                       the exemption

A buffered type is one in `lakelands-api` whose derive list names
`Deserialize` and whose serde attributes carry `flatten`, `untagged`,
`tag = ` or `content = `, unless the type's own attributes name `try_from`
or `from` (the derive then reads that other type, and the buffered
attributes belong to `Serialize`). It too may be exempt only with an issue
number.

`--check` fails on a site the list does not name, on a listed site that is
gone, on a buffered type, and on an exemption without an issue number.
`--write` regenerates the list from the tree, keeping existing verdicts.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITES = ROOT / "scripts" / "strict-doors.sites"

# The doors the record names, and the contract they read.
SCANNED = [
    "crates/lakelands-api/src",
    "crates/lakelands-server/src",
    "crates/lakelands-client/src",
    "crates/lakelands-cli/src",
    "crates/lakelands-backings/src",
    "crates/lakelands-core/src/bundle",
]

ENTRY_POINTS = [
    "serde_json::from_value",
    "serde_json::from_str",
    "serde_json::from_slice",
    "serde_json::from_reader",
    "serde_json::Deserializer::from_str",
    "serde_saphyr::from_str",
]

ISSUE = re.compile(r"^exempt lakelands#\d+")
VALUE_TYPE = re.compile(r"^(?:serde_json::)?Value$")

FN = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+)*fn\s+([A-Za-z0-9_]+)"
)
GENERIC = re.compile(re.escape("::<") + r"\s*([A-Za-z0-9_:]+)")

BUFFERED = re.compile(r"(?:\bflatten\b|\buntagged\b|\btag\s*=|\bcontent\s*=)")


def files():
    for top in SCANNED:
        for path in sorted((ROOT / top).rglob("*.rs")):
            rel = path.relative_to(ROOT).as_posix()
            if path.name == "tests.rs" or "/tests/" in rel or "/conformance/" in rel:
                continue
            if rel.endswith("/strict.rs"):
                continue
            yield rel, path.read_text()


def production(lines):
    """The lines before the file's own test module.

    `#[cfg(test)]` also guards a field or a method used only by tests; only
    a `mod tests` ends the production code."""
    for i, line in enumerate(lines):
        if line.strip() != "#[cfg(test)]":
            continue
        j = i + 1
        while j < len(lines) and not lines[j].strip():
            j += 1
        if j < len(lines) and re.match(r"\s*mod\s+tests\b", lines[j]):
            return lines[:i]
    return lines


def function_at(lines, index):
    for line in reversed(lines[: index + 1]):
        found = FN.match(line)
        if found:
            return found.group(1)
    return "<top>"


def function_span(lines, index):
    """The lines of the function that holds `index`, from its `fn` to its brace."""
    start = 0
    for i in range(index, -1, -1):
        if FN.match(lines[i]):
            start = i
            break
    depth, seen = 0, False
    for j in range(start, len(lines)):
        depth += lines[j].count("{") - lines[j].count("}")
        seen = seen or "{" in lines[j]
        if seen and depth <= 0:
            return lines[start : j + 1]
    return lines[start:]


def sites():
    found = []
    for rel, text in files():
        lines = production(text.splitlines())
        for i, line in enumerate(lines):
            code = line.split("//", 1)[0]
            for entry in ENTRY_POINTS:
                if entry not in code:
                    continue
                generic = GENERIC.search(code[code.index(entry) + len(entry) :])
                found.append(
                    {
                        "path": rel,
                        "line": i + 1,
                        "function": function_at(lines, i),
                        "entry": entry,
                        "generic": generic.group(1) if generic else None,
                        "strict": any(
                            "strict::" in body_line
                            for body_line in function_span(lines, i)
                        ),
                    }
                )
    return found


# -- buffered types ---------------------------------------------------------


def derive_blocks(lines):
    """Each `#[derive(...)]` line and the index of its item's first line."""
    for i, line in enumerate(lines):
        if not line.strip().startswith("#[derive("):
            continue
        block = [line]
        j = i + 1
        while j < len(lines) and lines[j].strip().startswith("#["):
            block.append(lines[j])
            j += 1
        yield i, " ".join(block), j


def item_body(lines, start):
    """The text of the item whose header is at `start` (attributes excluded)."""
    depth = 0
    seen = False
    body = []
    for line in lines[start:]:
        body.append(line)
        depth += line.count("{") - line.count("}")
        if "{" in line:
            seen = True
        if (seen and depth <= 0) or (not seen and line.rstrip().endswith(";")):
            return " ".join(body)
    return " ".join(body)


def buffered_types():
    found = []
    for rel, text in files():
        if not rel.startswith("crates/lakelands-api/src/"):
            continue
        lines = production(text.splitlines())
        for i, derive, j in derive_blocks(lines):
            if not re.search(r"\bDeserialize\b", derive):
                continue
            # The type's own attributes are the block's, and any after it.
            header = j
            own = [derive]
            while header < len(lines) and lines[header].strip().startswith("#["):
                own = own + [lines[header]]
                header += 1
            if header >= len(lines) or not re.match(
                r"\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|union)\b", lines[header]
            ):
                continue
            name = re.search(r"\b(?:struct|enum|union)\s+([A-Za-z0-9_]+)", lines[header])
            if not name:
                continue
            attrs = " ".join(own + [item_body(lines, header)])
            if not BUFFERED.search(attrs):
                continue
            if re.search(r"\b(?:try_from|from)\s*=", " ".join(own)):
                continue
            found.append({"path": rel, "line": i + 1, "type": name.group(1)})
    return found


# -- the list ---------------------------------------------------------------


def read_list():
    if not SITES.exists():
        return [], []
    entries, buffered = [], []
    for number, raw in enumerate(SITES.read_text().splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if parts[0] == "buffered" and len(parts) >= 4:
            buffered.append(
                {"path": parts[1], "type": parts[2], "verdict": " ".join(parts[3:])}
            )
        elif len(parts) >= 4:
            key = tuple(parts[:3])
            entries.append({"key": key, "verdict": " ".join(parts[3:]), "at": number})
        else:
            yield_error(f"line {number}: not `path function entry verdict`")
    return entries, buffered


def render(found_sites, found_buffered, old, old_buffered):
    """The list, in tree order, keeping each site's existing verdict.

    A function may hold the same call more than once, so a key's verdicts
    are kept in the order they were listed and taken one per call."""
    kept = {}
    for entry in old:
        kept.setdefault(entry["key"], []).append(entry["verdict"])
    out = [
        "# Every place a door crate or the contract reads: see scripts/strict-doors.py.",
        "# <path> <function> <entry point> <value|check|written|exempt lakelands#N>",
    ]
    for site in found_sites:
        key = (site["path"], site["function"], site["entry"])
        verdicts = kept.get(key, [])
        verdict = verdicts.pop(0) if verdicts else "TODO"
        out.append(" ".join(key) + "\t" + verdict)
    out.append("")
    out.append("# Types in the contract whose derived `Deserialize` buffers its input.")
    out.append("# buffered <path> <type> <exempt lakelands#N>")
    kept_buffered = {(item["path"], item["type"]): item["verdict"] for item in old_buffered}
    for item in found_buffered:
        verdict = kept_buffered.get((item["path"], item["type"]), "exempt lakelands#TODO")
        out.append(f"buffered {item['path']} {item['type']}\t{verdict}")
    return "\n".join(out) + "\n"


def yield_error(message):
    print(f"strict-doors: {message}", file=sys.stderr)


def main():
    mode = "--write" if "--write" in sys.argv else "--check"
    found = sites()
    buffered = buffered_types()
    entries, listed_buffered = read_list()
    bad = 0

    if mode == "--write":
        SITES.write_text(render(found, buffered, entries, listed_buffered))
        print(f"strict-doors: wrote {SITES.relative_to(ROOT)}")
        return 0

    seen = {}
    for site in found:
        key = (site["path"], site["function"], site["entry"])
        seen.setdefault(key, []).append(site)
    for entry in entries:
        key = entry["key"]
        site = seen[key].pop(0) if seen.get(key) else None
        if site is None:
            yield_error(f"list line {entry['at']}: nothing to list at {key[0]}")
            bad += 1
        verdict = entry["verdict"]
        if verdict not in ("value", "check", "written") and not ISSUE.match(verdict):
            yield_error(f"list line {entry['at']}: verdict `{verdict}` names no issue")
            bad += 1
        if verdict == "written" and site is not None and not site["strict"]:
            yield_error(
                f"list line {entry['at']}: `written` in `{key[1]}` of {key[0]} names no "
                "read through `strict`"
            )
            bad += 1
    for (path, function, entry), left in seen.items():
        if left:
            bad += 1
            yield_error(f"{path}: {entry} in `{function}` is not on the list")

    named = {(item["path"], item["type"]): item["verdict"] for item in listed_buffered}
    for item in buffered:
        verdict = named.pop((item["path"], item["type"]), None)
        if verdict is None:
            bad += 1
            yield_error(
                f"{item['path']}: `{item['type']}` derives a buffering `Deserialize`"
            )
        elif not ISSUE.match(verdict):
            bad += 1
            yield_error(
                f"{item['path']}: `{item['type']}` is exempt without an issue number"
            )
    for (path, name) in named:
        bad += 1
        yield_error(f"list names `{name}` in {path}, which does not buffer")

    if bad:
        print(f"strict-doors: RED ({bad} violation(s))", file=sys.stderr)
        return 1
    print(
        f"strict-doors: green ({len(found)} sites, {len(buffered)} exempt types, "
        f"{len(ENTRY_POINTS)} entry points)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
