# Port log

The core is imported from its source at one frozen commit,
`3d2cfc7b4dfb5348086cf1342aa1751846af4793`. The source keeps moving
after the freeze. Every change made to the storage core there after that
commit is listed here, one row each, and is either ported into Lakelands
or marked not applicable with the reason. The list is closed, and every
row settled, before the source platform switches to Lakelands.

| Source commit | Behaviour, in one line | Ported in, or not applicable and why |
|---|---|---|
| `5914084de9` | One German user-interface sentence in the capabilities module changes its form of address from formal to informal. | Not applicable: no storage behaviour changes — no event, no fold rule, no signature, no stored byte. The sentence is the platform's way of addressing its operator; Lakelands addresses no operator in German, so the import of that module restates or drops the platform's operator-facing prose rather than carrying it. |
