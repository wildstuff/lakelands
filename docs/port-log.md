# Port log

The core is imported from its source at one frozen commit,
`3d2cfc7b4dfb5348086cf1342aa1751846af4793`. The source keeps moving
after the freeze. Every change made to the storage core there after that
commit is listed here, one row each, and is either ported into Lakelands
or marked not applicable with the reason. The list is closed, and every
row settled, before the source platform switches to Lakelands.

**Checked through 2026-09-24**, against the source's mainline at
`3e22c2770b`: every commit after the freeze that touches a path an organ
was imported from so far, which is the storage core (contract and types,
the store, the engines), the expression grammar and value-format crates
folded into `lakelands-core`, the template loader's apply lane the
bundle format came from, and the tool layer the catalog was cut from.
One commit touches any of them; it is the row below. The check is run
again before O6 tags `0.1.0`, and again before the platform switches.

| Source commit | Behaviour, in one line | Ported in, or not applicable and why |
|---|---|---|
| `5914084de9` | One German user-interface sentence in the capabilities module changes its form of address from formal to informal. | Not applicable: no storage behaviour changes — no event, no fold rule, no signature, no stored byte. The sentence is the platform's way of addressing its operator; Lakelands addresses no operator in German, so the import of that module restates or drops the platform's operator-facing prose rather than carrying it. |
