#!/usr/bin/env python3
"""Writes walk 6's data files (`data/*.ndjson`) and prints the reference
answer of every task, computed from the same lists and not from the lake."""
import json
import sys
from decimal import Decimal
from pathlib import Path

SUPPLIERS = [
    ("S-1", "Nordic Fasteners", "SE"),
    ("S-2", "Lago Valvole", "IT"),
    ("S-3", "Rhein Pumpen", "DE"),
    ("S-4", "Torino Guarnizioni", "IT"),
]
PRODUCTS = [
    ("P-10", "Hex bolt M8", "fasteners", "0.40", "S-1"),
    ("P-11", "Lock nut M8", "fasteners", "0.25", "S-1"),
    ("P-12", "Washer M8", "fasteners", "0.10", "S-1"),
    ("P-13", "Threaded rod M8", "fasteners", "2.20", "S-1"),
    ("P-20", "Ball valve DN25", "valves", "38.00", "S-2"),
    ("P-21", "Check valve DN25", "valves", "42.50", "S-2"),
    ("P-30", "Centrifugal pump 2kW", "pumps", "640.00", "S-3"),
    ("P-31", "Pump seal kit", "pumps", "55.00", "S-3"),
    ("P-40", "O-ring set", "seals", "12.00", "S-4"),
    ("P-41", "Gasket DN25", "seals", "4.80", "S-4"),
    ("P-42", "Shaft seal 30mm", "seals", "19.00", "S-4"),
]
CUSTOMERS = [
    ("C-1", "Delta Metals", "DE", "industry"),
    ("C-2", "Harbor Supplies", "NL", "retail"),
    ("C-3", "Brightwater Utilities", "GB", "utility"),
    ("C-4", "Kestrel Farms", "IE", "retail"),
    ("C-5", "Ostrava Machining", "CZ", "industry"),
    ("C-6", "Vale Hardware", "PT", "retail"),
]
# (order, customer, placed on, status, [(product, qty), ...])
ORDERS = [
    ("O-1001", "C-1", "2026-08-03", "shipped", [("P-10", 400), ("P-11", 400), ("P-12", 800)]),
    ("O-1002", "C-2", "2026-08-11", "shipped", [("P-20", 6), ("P-41", 30)]),
    ("O-1003", "C-3", "2026-08-19", "shipped", [("P-30", 2), ("P-31", 4), ("P-21", 3)]),
    ("O-1004", "C-1", "2026-08-27", "open", [("P-20", 10), ("P-40", 25), ("P-41", 60), ("P-10", 150)]),
    ("O-1005", "C-5", "2026-09-02", "cancelled", [("P-30", 1)]),
    ("O-1006", "C-2", "2026-09-09", "open", [("P-10", 200), ("P-11", 250)]),
    ("O-1007", "C-4", "2026-09-15", "open", [("P-21", 2), ("P-40", 8)]),
    ("O-1008", "C-5", "2026-09-22", "shipped", [("P-12", 1200), ("P-31", 5)]),
    ("O-1009", "C-3", "2026-09-29", "open", [("P-30", 1), ("P-20", 4), ("P-41", 40)]),
    ("O-1010", "C-1", "2026-10-06", "open", [("P-11", 700), ("P-31", 2)]),
    ("O-1011", "C-2", "2026-10-13", "cancelled", [("P-40", 12)]),
    ("O-1012", "C-4", "2026-10-20", "open", [("P-10", 90), ("P-12", 300)]),
]

price = {p[0]: Decimal(p[3]) for p in PRODUCTS}
lines = []
for o, _, _, _, ls in ORDERS:
    for i, (p, q) in enumerate(ls, 1):
        lines.append((f"L-{o[2:]}-{i}", o, p, q))
total = {o[0]: sum(price[p] * q for p, q in o[4]) for o in ORDERS}


def write(out):
    rows = {
        "suppliers": [{"type": "supplier", "key": s, "fields": {"id": s, "name": n, "country": c}} for s, n, c in SUPPLIERS],
        "products": [{"type": "product", "key": k, "fields": {"sku": k, "name": n, "category": c, "unit_price": u, "supplier": s}} for k, n, c, u, s in PRODUCTS],
        "customers": [{"type": "customer", "key": k, "fields": {"id": k, "name": n, "country": c, "segment": g}} for k, n, c, g in CUSTOMERS],
        "orders": [{"type": "order", "key": o, "fields": {"id": o, "customer": c, "placed_on": d, "total": f"{total[o]:.2f}", "status": s}} for o, c, d, s, _ in ORDERS],
        "order_lines": [{"type": "order_line", "key": k, "fields": {"id": k, "order": o, "product": p, "qty": q}} for k, o, p, q in lines],
    }
    for name, rs in rows.items():
        (out / f"{name}.ndjson").write_text("".join(json.dumps(r) + "\n" for r in rs))


def answers():
    cust = {c[0]: c for c in CUSTOMERS}
    prod = {p[0]: p for p in PRODUCTS}
    sup = {s[0]: s for s in SUPPLIERS}
    order = {o[0]: o for o in ORDERS}
    print("1. Harbor Supplies' orders:")
    for o in ORDERS:
        if o[1] == "C-2":
            print(f"   {o[0]} {o[2]} {total[o[0]]:.2f} {o[3]}")
    print("2. On O-1007:")
    for k, o, p, q in lines:
        if o == "O-1007":
            print(f"   {prod[p][1]} ({p}) x {q}")
    print("3. Suppliers behind O-1004:")
    for s in sorted({prod[p][4] for k, o, p, q in lines if o == "O-1004"}):
        print(f"   {sup[s][1]} ({s}) {sup[s][2]}")
    print("4. Open orders with a valve:")
    print("  ", sorted({o for k, o, p, q in lines if prod[p][2] == "valves" and order[o][3] == "open"}))
    print("5. Orders and value per customer:")
    for c in CUSTOMERS:
        os_ = [o for o in ORDERS if o[1] == c[0]]
        print(f"   {c[1]}: {len(os_)} orders, {sum((total[o[0]] for o in os_), Decimal(0)):.2f}")
    print("6. Units per category, cancelled orders left out:")
    units = {}
    for k, o, p, q in lines:
        if order[o][3] != "cancelled":
            units[prod[p][2]] = units.get(prod[p][2], 0) + q
    for cat in sorted(units):
        print(f"   {cat}: {units[cat]}")
    print("7. Order lines by quantity, ranks 6 to 10:")
    ranked = sorted(lines, key=lambda l: -l[3])
    for r, (k, o, p, q) in enumerate(ranked, 1):
        if 5 <= r <= 11:
            print(f"   {r}. {k} {o} {p} {q}")
    print("8. Shortest chains from Kestrel Farms (C-4) to Ostrava Machining (C-5):")
    edges = {}
    for k, o, p, q in lines:
        for a, b in ((k, o), (k, p)):
            edges.setdefault(a, set()).add(b)
            edges.setdefault(b, set()).add(a)
    for o in ORDERS:
        edges.setdefault(o[0], set()).add(o[1])
        edges.setdefault(o[1], set()).add(o[0])
    for p in PRODUCTS:
        edges.setdefault(p[0], set()).add(p[4])
        edges.setdefault(p[4], set()).add(p[0])
    paths, frontier = [], [["C-4"]]
    while frontier and not paths:
        frontier = [f + [n] for f in frontier for n in sorted(edges[f[-1]]) if n not in f]
        paths = [f for f in frontier if f[-1] == "C-5"]
    for f in paths:
        print(f"   {len(f) - 1} links:", " - ".join(f))
    print("9. Never ordered:", sorted(set(prod) - {p for _, _, p, _ in lines}))


if __name__ == "__main__":
    if sys.argv[1:] == ["write"]:
        write(Path(__file__).parent / "data")
    else:
        answers()
