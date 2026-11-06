#!/usr/bin/env python3
"""Prints the reference answer of each task walk 7 adds to walk 6's ten,
computed from the lists walk 6's `make_data.py` writes the lake from, not
from the lake. Walk 6's own ten: `python3 docs/walks/walk-6/lake/make_data.py`."""
import importlib.util
from decimal import Decimal
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "make_data", Path(__file__).resolve().parent.parent / "walk-6" / "lake" / "make_data.py")
d = importlib.util.module_from_spec(spec)
spec.loader.exec_module(d)

cust = {c[0]: c for c in d.CUSTOMERS}
prod = {p[0]: p for p in d.PRODUCTS}
sup = {s[0]: s for s in d.SUPPLIERS}
order = {o[0]: o for o in d.ORDERS}
by_name = {c[1]: c[0] for c in d.CUSTOMERS} | {s[1]: s[0] for s in d.SUPPLIERS}


def lines_of(c):
    return [l for l in d.lines if order[l[1]][1] == c]


print("11. The maker of the goods Kestrel Farms bought most units of:")
units = {}
for k, o, p, q in lines_of("C-4"):
    units[prod[p][4]] = units.get(prod[p][4], 0) + q
for s, n in sorted(units.items(), key=lambda x: -x[1]):
    print(f"   {sup[s][1]} ({s}) {sup[s][2]}: {n} units")

print("12. Customers who bought anything made by Rhein Pumpen:")
got = sorted({order[o][1] for k, o, p, q in d.lines if prod[p][4] == "S-3"})
print("  ", [f"{c} {cust[c][1]}" for c in got])

print("13. Delta Metals' orders, their sum, and those not shipped:")
mine = [o for o in d.ORDERS if o[1] == "C-1"]
for o in mine:
    print(f"   {o[0]} {o[3]} {d.total[o[0]]:.2f}")
print(f"   all: {sum(d.total[o[0]] for o in mine):.2f}; open: {sum(d.total[o[0]] for o in mine if o[3] == 'open'):.2f}")

print("14. Lines of Lago Valvole's products, largest quantity first:")
for k, o, p, q in sorted((l for l in d.lines if prod[l[2]][4] == "S-2"), key=lambda l: (-l[3], l[0])):
    print(f"   {k} {o} {p} {q}")

print("15. Orders holding goods made by Torino Guarnizioni:")
for o in sorted({o for k, o, p, q in d.lines if prod[p][4] == "S-4"}):
    print(f"   {o} {order[o][1]} {cust[order[o][1]][1]} {order[o][3]}")

print("16. Shortest chains from Harbor Supplies (C-2) to Rhein Pumpen (S-3):")
edges = {}
def link(a, b):
    edges.setdefault(a, set()).add(b)
    edges.setdefault(b, set()).add(a)
for k, o, p, q in d.lines:
    link(k, o)
    link(k, p)
for o in d.ORDERS:
    link(o[0], o[1])
for p in d.PRODUCTS:
    link(p[0], p[4])
paths, frontier = [], [["C-2"]]
while frontier and not paths:
    frontier = [f + [n] for f in frontier for n in sorted(edges[f[-1]]) if n not in f]
    paths = [f for f in frontier if f[-1] == "S-3"]
for f in paths:
    print(f"   {len(f) - 1} links:", " - ".join(f))

print("17. Open orders over 12,000.00 placed by customers in Ireland:")
for o in d.ORDERS:
    if o[3] == "open" and cust[o[1]][2] == "IE" and d.total[o[0]] > Decimal("12000"):
        print(f"   {o[0]} {o[1]} {cust[o[1]][1]} {d.total[o[0]]:.2f}")
