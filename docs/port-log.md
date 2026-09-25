# Port log

The core is imported from its source at one frozen commit,
`3d2cfc7b4dfb5348086cf1342aa1751846af4793`. The source keeps moving
after the freeze. Every change made to the storage core there after that
commit is listed here, one row each, and is either ported into Lakelands
or marked not applicable with the reason. The list is closed, and every
row settled, before the source platform switches to Lakelands.

**Checked through 2026-09-24, for `0.1.0`**, against the source's mainline
at `a789d52b23`: every commit after the freeze (123 of them) that touches
a path an organ was imported from, which is the storage core (contract
and types, the store, the engines), the expression grammar and
value-format crates folded into `lakelands-core`, the template loader's
apply lane the bundle format came from, and the tool layer the catalog
was cut from (`docs/import-ledger.md` lists every one). One commit touches
any of them; it is the row below, settled. **The list is closed for
`0.1.0`.** It is checked again before the platform switches.

**The `{pg}` root, checked on 2026-09-24 for lakelands#65**, when the
PostgreSQL engine was taken from it, against the source's mainline at
`87e79584fc`: no commit after the freeze touches it, so it adds no row.

**Checked again on 2026-09-24, for `0.2.0`**, against the source's
mainline at `3cea3c6a4f`: every commit after the freeze (157 of them)
against all nine roots of `docs/import-ledger.md`, `{pg}` included, and
for `{tools}` the ten files the catalog was cut from. Each root holds
the same files it held at the freeze, so no change was hidden by a
move. One commit touches any of them, and it is still the row below.
**The list is closed for `0.2.0` as of that commit**; if the mainline
moves before the tag, the check is run again first.

**Checked again on 2026-09-25, before the `0.2.0` tag**, against the
source's mainline at `c0b30a3f2e`: the mainline had moved, and the 14
commits after `3cea3c6a4f` (171 after the freeze in all) were checked
the same way. None of them touches a root, each root still holds the
files it held at the freeze, and Lakelands has taken nothing new from
the source since. The row below is still the only one. **The list is
closed for `0.2.0` as of `c0b30a3f2e`.**

**Checked again on 2026-09-25, a second time before the `0.2.0` tag**,
against the source's mainline at `db4b2949e7`: the 113 commits after
`c0b30a3f2e` (284 after the freeze in all) were checked against all nine
roots, `{tools}` by its ten files. None of them touches a root, each
root still holds the files it held at the freeze, and Lakelands has
taken nothing new from the source since. The row below is still the only
one. **The list is closed for `0.2.0` as of `db4b2949e7`**; on the day of
the tag only the commits after it are checked, if the mainline has moved.

| Source commit | Behaviour, in one line | Ported in, or not applicable and why |
|---|---|---|
| `5914084de9` | One German user-interface sentence in the capabilities module changes its form of address from formal to informal. | Not applicable: the capabilities module was not carried (cut in O2: there is no capability negotiation; see `docs/import-ledger.md`), and no storage behaviour changes: no event, no fold rule, no signature, no stored byte. |
