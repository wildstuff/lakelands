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

| Source commit | Behaviour, in one line | Ported in, or not applicable and why |
|---|---|---|
| `5914084de9` | One German user-interface sentence in the capabilities module changes its form of address from formal to informal. | Not applicable: the capabilities module was not carried (cut in O2: there is no capability negotiation; see `docs/import-ledger.md`), and no storage behaviour changes: no event, no fold rule, no signature, no stored byte. |
