#!/usr/bin/env python3
"""Writes docs/settings.md from the list of settings, and holds the list
to the code that reads it.

Usage: scripts/settings.py [--check | --write]

The list is `SETTINGS` in `crates/lakelands-api/src/settings.rs`: every
setting that steers a lake, with where it is kept, its default, what
values it takes, what it changes and what it costs
(`docs/decisions/063-every-setting-is-in-one-list.md`). The page is
generated, never written by hand, so it cannot drift from the list.

--check (the default, what the `settings` gate runs) exits 1 when:

- **the page is not the list:** `docs/settings.md` differs from what
  the list says;
- **a key the store reads is not listed:** a `pub const <NAME>_SETTING`
  of `crates/lakelands-api/src/seam.rs`, the prefix of a lake setting
  the store reads, names no listed lake setting;
- **a listed lake setting is read nowhere:** a lake setting of the list
  has no such constant, or its constant is used by no product file
  outside `seam.rs` and `settings.rs`;
- **a setting is read by a literal:** a product file outside `seam.rs`
  and `settings.rs` holds a string literal that is a listed lake
  setting's key or begins with one and a dot, or reads the settings
  (`settings.get`, `.contains_key`, `.range`, `setting_for`) with a
  literal key, a `format!` of one included, a method `settings()` and a
  closure over them (`settings.and_then(|m| m.get(..))`) included, or
  tests a key of a walk of them against a literal prefix
  (`settings.keys().any(|k| k.starts_with(..))`). Reads are matched
  across lines, so a chain rustfmt splits is seen whole. A key the store reads is a constant of
  `seam.rs`, so the checks above see it;
- **a setting's constant is held elsewhere:** a product file outside
  `seam.rs` declares a `const <NAME>_SETTING`;
- **the settings are read under another name:** a product file binds
  them to a name other than `settings`, where a read is not seen.

A location's and the root's settings and bindings are held to
`Location`, `Limits`, the engines' tuning and the root's sections by
unit tests, both ways (`crates/lakelands-api/src/settings/tests.rs`,
`crates/lakelands-cli/tests/cli.rs`).

--write writes the page.
"""

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LIST = ROOT / "crates/lakelands-api/src/settings.rs"
SEAM = ROOT / "crates/lakelands-api/src/seam.rs"
PAGE = ROOT / "docs/settings.md"

STR = r'"((?:[^"\\]|\\.)*)"'
ENTRY = re.compile(r"Setting \{(.*?)\.\.(lake|location|root)\(" + r"(.*?)\)\s*,?\s*\}", re.S)
BINDING = re.compile(r"Binding \{\s*key:\s*" + STR + r",\s*layer:\s*Layer::(\w+),\s*says:\s*" + STR, re.S)
LITERAL = re.compile(STR)
# A read of the settings by a literal key: `get`, `contains_key` or
# `range` on them, a method `settings()` included, through a closure over
# them, or a prefix test in a walk of their keys; and `setting_for`.
READ = re.compile(
    r"(?:\bsettings\s*(?:\(\s*\))?\s*\??\s*\.\s*(?:get|contains_key|range)\s*\(\s*&?\s*(?:format!\s*\(\s*)?"
    r"|\bsettings\s*(?:\(\s*\))?\s*\??\s*\.\s*(?:and_then|map)\s*\(\s*\|\s*\w+\s*\|\s*\w+\s*\.\s*"
    r"(?:get|contains_key|range)\s*\(\s*&?\s*(?:format!\s*\(\s*)?"
    r"|\bsettings\s*(?:\(\s*\))?\s*\??\s*\.\s*(?:keys|iter|into_iter)\s*\(\s*\)[^;{}]*?"
    r"\b(?:starts_with|strip_prefix|ends_with|contains)\s*\(\s*"
    r"|\bsetting_for\s*\([^,()]*,\s*)" + STR
)
# A constant of a setting's key: only seam.rs holds one.
OWN_CONST = re.compile(r"\bconst\s+(\w+_SETTING)\s*:")
# The settings bound to a name other than `settings`, where a read of
# them would not be seen.
ALIAS = re.compile(
    r"\blet\s+(?:mut\s+)?(?!settings\b)\w+\s*(?::[^=;]*)?=\s*&?\s*(?:[\w.]+\s*\.\s*)?settings\s*;"
)
MEMBER = re.compile(r"(range|says|cost):\s*" + STR, re.S)
CONST = re.compile(r"pub const (\w+_SETTING): &str = " + STR + ";")

LAYERS = {
    "lake": (
        "Kept in the lake",
        "A record of `sys.setting`, in the lake's log: every host that opens the lake "
        "finds it the same, and an auditor reads its history. Set it with "
        "`lakelands settings set <key> <value>` (`--type <type>` for one type), or any "
        "write of `sys.setting`; retire it with `lakelands settings unset`, and it reads "
        "its default again. A value the lake cannot use is refused, naming why.",
    ),
    "location": (
        "Kept in the lake's location",
        "A member of `lakes/<id>.json`, read when the lake is opened: a bound on what one "
        "host spends, which changes no answer. Edit the file and open the lake again.",
    ),
    "root": (
        "Kept in the root's configuration",
        "A member of `lakelands.json`, read when a process opens the root (`server` when "
        "`serve` starts): how the root's processes serve, age, deliver, embed, call a "
        "backing and mirror. Edit the "
        "file and open the root again.",
    ),
}


def unquote(s):
    return bytes(s, "utf-8").decode("unicode_escape").encode("latin-1").decode("utf-8")


def listed():
    text = LIST.read_text()
    body = text[text.index("pub const SETTINGS"):]
    out = []
    for m in ENTRY.finditer(body):
        members = {k: unquote(v) for k, v in MEMBER.findall(m.group(1))}
        args = [unquote(a) for a in re.findall(STR, m.group(3))]
        per_type = re.search(r"PerType::(\w+)", m.group(3))
        out.append({
            "layer": m.group(2),
            "key": args[0],
            "default": args[-1],
            "per_type": per_type.group(1) if per_type else "No",
            **members,
        })
    return out


def bindings():
    text = LIST.read_text()
    body = text[text.index("pub const BINDINGS"):]
    return [
        {"key": unquote(k), "layer": layer.lower(), "says": unquote(says)}
        for k, layer, says in BINDING.findall(body)
    ]


def page(settings, bound):
    lines = [
        "# Settings",
        "",
        "<!-- Generated by scripts/settings.py from crates/lakelands-api/src/settings.rs; "
        "do not edit. -->",
        "",
        "Every setting that steers a lake, and where it is kept. A setting lives where what "
        "it steers lives: what every host that opens the lake must find the same is kept in "
        "the lake; a bound on what one host spends, in the lake's location; how the root's "
        "processes serve, age, deliver, embed, call a backing and mirror, in the root's "
        "`lakelands.json` "
        "(`docs/decisions/063-every-setting-is-in-one-list.md`).",
        "",
        "`lakelands settings` shows each one with the value it has for a lake. A host's own "
        "keys under `sys.setting`, which its expressions read as `setting('<key>')`, are its "
        "own and not listed here; a key under the prefix of a listed lake setting that names "
        "none of them is refused.",
    ]
    for layer, (title, intro) in LAYERS.items():
        lines += ["", f"## {title}", "", intro]
        for s in (s for s in settings if s["layer"] == layer):
            key = s["key"]
            shown = {"Only": f"{key}.<type>", "Override": f"{key}` and `{key}.<type>"}
            lines += ["", f"### `{shown.get(s['per_type'], key)}`", "", s["says"], ""]
            lines.append(f"- **Takes:** {s['range']}.")
            lines.append(f"- **Default:** `{s['default']}`.")
            if s["per_type"] == "Only":
                lines.append("- **Per type:** one value per type, and none for the lake.")
            elif s["per_type"] == "Override":
                lines.append(
                    f"- **Per type:** `{key}.<type>` overrides `{key}` for that type; "
                    "retired, the type has the lake's value again."
                )
            if s.get("cost"):
                lines.append(f"- **Cost:** {s['cost']}.")
    lines += [
        "",
        "## Bindings",
        "",
        "What says only where or how to connect, or which keys to use, is a binding and no "
        "setting: it steers no behaviour of the lake, and a lake moved or restored names it "
        "anew. Each is kept beside the settings of its layer. The rest of `lakelands.json` is "
        "neither: `lakelands` is the file's format number, and `owner` and `principals` say "
        "who the root's callers are and what each may do.",
        "",
    ]
    for b in bound:
        where = "the location" if b["layer"] == "location" else "`lakelands.json`"
        lines.append(f"- **`{b['key']}`** ({where}): {b['says']}")
    return "\n".join(lines) + "\n"


def product_files():
    files = subprocess.run(
        ["git", "ls-files", "crates/*/src/*.rs", "crates/*/src/**/*.rs"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout.split()
    spec = re.compile(r"(^|/)(tests\.rs|conformance/|tests/)|^crates/lakelands-door-checks/")
    return [f for f in files if not spec.search(f) and f not in (str(SEAM.relative_to(ROOT)), str(LIST.relative_to(ROOT)))]


def product_text(f):
    """A product file's text, without the tests a crate outside the core
    keeps inline at its end."""
    text = (ROOT / f).read_text()
    inline = re.search(r"^#\[cfg\(test\)\]\s*\nmod tests \{", text, re.M)
    return text[: inline.start()] if inline else text


def violations(settings, bound):
    out = []
    lake = {s["key"] for s in settings if s["layer"] == "lake"}
    consts = {name: value.rstrip(".") for name, value in CONST.findall(SEAM.read_text())}
    for name, key in sorted(consts.items()):
        if key not in lake:
            out.append(f"settings: `{name}` (`{key}`) is a setting the store reads and not listed")
    sources = {f: product_text(f) for f in product_files()}
    # A key with its type, or a lake-wide key: a per-type setting's bare
    # name is no key, and a table may be called by it.
    keyed = [(k, s["per_type"] != "Only") for s in settings if s["layer"] == "lake" for k in [s["key"]]]
    for f, text in sorted(sources.items()):
        # A comment line is blanked, not dropped, so a match keeps its line.
        lines = ["" if l.lstrip().startswith("//") else l for l in text.splitlines()]
        for n, line in enumerate(lines, 1):
            for lit in LITERAL.findall(line):
                if any((bare and lit == k) or lit.startswith(k + ".") for k, bare in keyed):
                    out.append(f"settings: {f}:{n} names the setting `{lit}` by a literal; name it by its constant in seam.rs")
            for name in OWN_CONST.findall(line):
                out.append(f"settings: {f}:{n} holds the constant `{name}`; a setting's key is a constant of seam.rs")
        # Reads are matched on the whole file, so a chain rustfmt splits
        # over lines is seen whole.
        code = "\n".join(lines)
        at = lambda m: code.count("\n", 0, m.start()) + 1
        for m in READ.finditer(code):
            if m.group(1).split("{")[0] == "":
                continue
            out.append(f"settings: {f}:{at(m)} reads the setting `{m.group(1)}` by a literal; make its key a constant in seam.rs")
        for m in ALIAS.finditer(code):
            said = re.sub(r"\s+", " ", m.group(0))
            out.append(f"settings: {f}:{at(m)} binds the settings to another name (`{said}`); read them where they are named, so this check sees the key")
    for key in sorted(lake):
        names = [n for n, k in consts.items() if k == key]
        if not names:
            out.append(f"settings: `{key}` is listed and has no `*_SETTING` constant the store reads")
        elif not any(re.search(rf"\b{n}\b", t) for n in names for t in sources.values()):
            out.append(f"settings: `{key}` is listed and no product file reads `{names[0]}`")
    if not PAGE.exists() or PAGE.read_text() != page(settings, bound):
        out.append("settings: docs/settings.md is not what the list says; run scripts/settings.py --write")
    return out


def main(argv):
    settings, bound = listed(), bindings()
    if not settings:
        print("settings: read no setting from the list", file=sys.stderr)
        return 2
    if argv[1:] == ["--write"]:
        PAGE.write_text(page(settings, bound))
        print(f"settings: wrote docs/settings.md: {len(settings)} settings")
        return 0
    if argv[1:] not in ([], ["--check"]):
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    bad = violations(settings, bound)
    for line in bad:
        print(line)
    if bad:
        print(f"settings: RED ({len(bad)} violation(s))")
        return 1
    print(f"settings: green ({len(settings)} settings)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
