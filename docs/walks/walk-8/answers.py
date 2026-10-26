#!/usr/bin/env python3
"""Prints the reference answer of each of walk 8's tasks, computed from
the lists `lake/make_data.py` writes the lake from, not from the lake.
Task 9's ranking is BM25F as the lake scores a text (`docs/seams.md`
§`ranked`), over the lists: the type's searchable fields in declaration
order, each of weight 1, K1 1.2 and B 0.75, idf ln((2N+2)/(2df+1)) with N
the sum over those fields of the records holding a token there; a token
is a run of letters and digits, lower-cased, its diacritics folded, as a
type that declares no language tokenises it."""
import importlib.util
import math
import re
import unicodedata
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "make_data", Path(__file__).resolve().parent / "lake" / "make_data.py")
d = importlib.util.module_from_spec(spec)
spec.loader.exec_module(d)

cust = {c[0]: c for c in d.CUSTOMERS}
prod = {p[0]: p for p in d.PRODUCTS}
sup = {s[0]: s for s in d.SUPPLIERS}
order = {o[0]: o for o in d.ORDERS}
C, P, S = d.PLANTED_CUST, d.PLANTED_SKU, d.SUP_BY_NAME


def tokens(text):
    folded = unicodedata.normalize("NFD", text.lower())
    folded = "".join(c for c in folded if not unicodedata.combining(c))
    return re.findall(r"[^\W_]+", folded)


def bm25f(rows, fields, text, k1=1.2, b=0.75):
    """The hits of `text` over `rows`, each a key and its fields' texts,
    best first, ties by key: (key, score, distinct query tokens held,
    query tokens held, repeats counted)."""
    query = sorted(set(tokens(text)))
    held = {key: [tokens(t) for t in texts] for key, texts in rows}
    n_f = [sum(1 for h in held.values() if h[i]) for i in range(len(fields))]
    l_f = [sum(len(h[i]) for h in held.values()) for i in range(len(fields))]
    n = sum(n_f)
    df = {t: sum(1 for h in held.values() if any(t in f for f in h)) for t in query}
    hits = []
    for key, h in held.items():
        score, distinct, repeats = 0.0, 0, 0
        for t in query:
            x, any_ = 0.0, False
            for i, f in enumerate(h):
                tf = f.count(t)
                if not tf:
                    continue
                any_ = True
                avg = l_f[i] / n_f[i]
                x += tf / (1 - b + b * len(f) / avg)
                repeats += tf
            if any_:
                distinct += 1
                score += math.log((2 * n + 2) / (2 * df[t] + 1)) * x * (k1 + 1) / (x + k1)
        if distinct:
            hits.append((key, score, distinct, repeats))
    return sorted(hits, key=lambda h: (-h[1], h[0]))


def name(c):
    return f"{c} {cust[c][1]} ({cust[c][2]})"


print("1. Kestral Farms: no customer is so called; the one near it is", name(C["kestrel"]))
mine = [o for o in d.ORDERS if o[1] == C["kestrel"]]
for o in mine:
    print(f"   {o[0]} {o[2]} {o[3]} {d.total[o[0]]:.2f}")
print(f"   {len(mine)} orders, {sum(d.total[o[0]] for o in mine):.2f} in all")

t = S["Torino Guarnizioni"]
made = [p for p in d.PRODUCTS if p[5] == t]
print(f"2. Torino Guarnizoni: no supplier is so called; the one near it is {t} {sup[t][1]}, in {sup[t][2]}; "
      f"{len(made)} products: {', '.join(p[0] for p in made)}")

print("3. Atlas Supplies in Portugal,", name(C["atlas-pt"]), "- units by product, cancelled orders left out:")
units = {}
for k, o, p, q in d.lines:
    if order[o][1] == C["atlas-pt"] and order[o][3] != "cancelled":
        units[p] = units.get(p, 0) + q
for p, u in sorted(units.items(), key=lambda x: (-x[1], x[0])):
    print(f"   {p} {prod[p][1]}: {u}")

print("4. Atlas Supplies: three customers are so called; each one's last order:")
for tag in ("atlas-nl", "atlas-pt", "atlas-gb"):
    last = max((o for o in d.ORDERS if o[1] == C[tag]), key=lambda o: o[2])
    print(f"   {name(C[tag])}: {last[0]} on {last[2]}, {last[3]}")

p = prod[P["cellar"]]
print(f"5. The pump for flooded cellars: {p[0]} {p[1]}, made by {p[5]} {sup[p[5]][1]}, {p[4]}")
print(f"   other products called {p[1]}:", [q[0] for q in d.PRODUCTS if q[1] == p[1] and q[0] != p[0]])

p = P["brewery"]
print(f"6. Customers who bought {p} {prod[p][1]} (for breweries and dairies), units, cancelled orders left out:")
by = {}
for k, o, q, n in d.lines:
    if q == p and order[o][3] != "cancelled":
        by[order[o][1]] = by.get(order[o][1], 0) + n
for c, n in sorted(by.items(), key=lambda x: (-x[1], x[0])):
    print(f"   {name(c)}: {n}")
cancelled = sorted({order[o][1] for k, o, q, n in d.lines if q == p and order[o][3] == "cancelled"} - set(by))
print("   on cancelled orders only:", [name(c) for c in cancelled])

b = S["Bellagio Valvole"]
print(f"7. Customers in Germany who ordered goods of {b} {sup[b][1]} (Lake Como), every status:")
got = {}
for k, o, q, n in d.lines:
    if prod[q][5] == b and cust[order[o][1]][2] == "DE":
        got.setdefault(order[o][1], set()).add(o)
for c in sorted(got, key=lambda c: int(c[2:])):
    print(f"   {name(c)}: {', '.join(sorted(got[c]))}")

g = S["Baltic Okucia"]
units = sum(n for k, o, q, n in d.lines if prod[q][5] == g and order[o][3] == "open")
print(f"8. Units of {g} {sup[g][1]} (rail from Gdansk) goods on open orders: {units}")

rows = [(p[0], (p[1], p[2])) for p in d.PRODUCTS]
hits = bm25f(rows, ("name", "description"), "seawater pump")
print("9. 'seawater pump', BM25F over name and description, the first five:")
for key, score, distinct, repeats in hits[:5]:
    print(f"   {key} {prod[key][1]}: {score:.4f} ({distinct} distinct, {repeats} held)")
print("   by distinct tokens held, ties by key:", sorted(hits, key=lambda h: (-h[2], h[0]))[0][0])
print("   by tokens held, repeats counted:", sorted(hits, key=lambda h: (-h[3], h[0]))[0][0])
for f, i in (("name", 0), ("description", 1)):
    one = bm25f([(p[0], (p[1 + i],)) for p in d.PRODUCTS], (f,), "seawater pump")
    print(f"   BM25F over {f} alone, the first three:", [(k, round(s, 4)) for k, s, _, _ in one[:3]])

print("10. Quillon Aerospace: no customer is so called, and none near it;",
      sum("aerospace" in c[1].lower() for c in d.CUSTOMERS), "customers' names hold Aerospace")
